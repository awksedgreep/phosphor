//! The scripting hook (DESIGN.md rule 1/2/3): a small, sandboxed Lua
//! environment that talks to the database only through [`DbLink`] and
//! exchanges the one [`PValue`] type — never a second conversion layer.
//!
//! Slice 1 exposes three globals to a script:
//!
//!   query(sql)    -> a sequence of row tables keyed by column name
//!   execute(sql)  -> affected-row count (-1 for a batch)
//!   say(value)    -> append a line to the run's report
//!
//! The script itself is stored inline in an app menu item's `action_ref`
//! (one line), so the Applications Generator needs no new table. Form
//! lifecycle events (`OnValidate`/`OnSave`) and emitting `Command`s are
//! the deliberate next slice; the bus already routes those moments.

use std::cell::RefCell;

use mlua::{Lua, Value};

use crate::db::{DbLink, PValue};

fn to_lua(lua: &Lua, v: &PValue) -> mlua::Result<Value> {
    Ok(match v {
        PValue::Null => Value::Nil,
        PValue::Int(i) => Value::Integer(*i),
        PValue::Real(f) => Value::Number(*f),
        PValue::Text(t) => Value::String(lua.create_string(t)?),
        PValue::Blob(b) => Value::String(lua.create_string(b)?),
    })
}

fn lua_to_string(v: &Value) -> String {
    match v {
        Value::Nil => "nil".to_owned(),
        Value::Boolean(b) => b.to_string(),
        Value::Integer(i) => i.to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => s.to_string_lossy().to_string(),
        other => format!("{other:?}"),
    }
}

/// Run `source` against `db`. Returns the `say`/return-value transcript.
/// A hard instruction budget and memory cap keep a bad script from
/// hanging the terminal.
pub fn run(db: &dyn DbLink, source: &str) -> Result<String, String> {
    let lua = Lua::new();
    // 32 MB of Lua heap is plenty for a menu action.
    let _ = lua.set_memory_limit(32 * 1024 * 1024);
    lua.set_hook(
        mlua::HookTriggers::new().every_nth_instruction(200_000),
        |_lua, _debug| Err(mlua::Error::RuntimeError("script exceeded budget".into())),
    );

    let output: RefCell<Vec<String>> = RefCell::new(Vec::new());
    let scope_result: mlua::Result<()> = lua.scope(|scope| {
        let query = scope.create_function(|lua, sql: String| {
            let q = db.query(&sql).map_err(mlua::Error::RuntimeError)?;
            let table = lua.create_table()?;
            for (ri, row) in q.rows.iter().enumerate() {
                let rt = lua.create_table()?;
                for (ci, cell) in row.iter().enumerate() {
                    if let Some(name) = q.columns.get(ci) {
                        rt.set(name.as_str(), to_lua(lua, cell)?)?;
                    }
                }
                table.set(ri + 1, rt)?;
            }
            Ok(table)
        })?;
        let execute = scope.create_function(|_, sql: String| {
            let (n, _) = db.execute(&sql).map_err(mlua::Error::RuntimeError)?;
            Ok(n)
        })?;
        let say = scope.create_function(|_, msg: Value| {
            output.borrow_mut().push(lua_to_string(&msg));
            Ok(())
        })?;
        let globals = lua.globals();
        globals.set("query", query)?;
        globals.set("execute", execute)?;
        globals.set("say", say)?;
        // A trailing expression is reported too, so `return #rows` works.
        let ret: Value = lua.load(source).eval()?;
        if !matches!(ret, Value::Nil) {
            output.borrow_mut().push(lua_to_string(&ret));
        }
        Ok(())
    });
    match scope_result {
        Ok(()) => {
            let mut lines = output.into_inner();
            if lines.is_empty() {
                lines.push("script: ok".to_owned());
            }
            Ok(lines.join("\n"))
        }
        Err(e) => Err(e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::EmbeddedDb;

    fn db() -> EmbeddedDb {
        let (db, _) = EmbeddedDb::open(":memory:").unwrap();
        db.execute("CREATE TABLE t(id INTEGER PRIMARY KEY, name TEXT)")
            .unwrap();
        db
    }

    #[test]
    fn execute_then_query() {
        let db = db();
        let msg = run(
            &db,
            r#"
            local n = execute("INSERT INTO t(name) VALUES ('ada'), ('grace')")
            say("inserted " .. n)
            local rows = query("SELECT name FROM t ORDER BY name")
            say("first " .. rows[1].name)
            "#,
        )
        .unwrap();
        assert!(msg.contains("inserted 2"), "{msg}");
        assert!(msg.contains("first ada"), "{msg}");
    }

    #[test]
    fn nil_and_numbers_round_trip() {
        let db = db();
        db.execute("INSERT INTO t(name) VALUES (NULL)").unwrap();
        let msg = run(
            &db,
            r#"
            local r = query("SELECT name FROM t")
            say(r[1].name == nil and "null" or "not-null")
            say(6 * 7)
            "#,
        )
        .unwrap();
        assert!(msg.contains("null"), "{msg}");
        assert!(msg.contains("42"), "{msg}");
    }

    #[test]
    fn sql_errors_surface_cleanly() {
        let db = db();
        let err = run(&db, r#"execute("SELECT nope FROM t")"#).unwrap_err();
        assert!(err.contains("no such column"), "{err}");
    }

    #[test]
    fn runaway_loop_is_stopped() {
        let db = db();
        let err = run(&db, "while true do end").unwrap_err();
        assert!(err.contains("budget"), "{err}");
    }
}

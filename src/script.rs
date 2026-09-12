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

use crate::db::{DbLink, DbResult, PValue};
use crate::store;

/// The lifecycle moments a form can subscribe to (DESIGN.md rule 4).
pub const EVENTS: [&str; 3] = ["OnValidate", "OnSave", "OnChange"];

/// Accept `validate`/`OnValidate`/`onvalidate` (any case) and friends.
pub fn normalize_event(s: &str) -> Option<&'static str> {
    match s.to_ascii_lowercase().as_str() {
        "onvalidate" | "validate" => Some("OnValidate"),
        "onsave" | "save" => Some("OnSave"),
        "onchange" | "change" => Some("OnChange"),
        _ => None,
    }
}

/// The Lua source bound to (table, event), if any. Read-only path.
pub fn get_script(db: &dyn DbLink, table: &str, event: &str) -> Option<String> {
    let q = db
        .query(&format!(
            "SELECT source FROM _phosphor_scripts WHERE table_ref = {} AND event = {} LIMIT 1",
            store::q(table),
            store::q(event)
        ))
        .ok()?;
    match q.rows.into_iter().next()?.into_iter().next()? {
        PValue::Text(t) => Some(t),
        _ => None,
    }
}

/// Bind (or replace) a script for (table, event).
pub fn set_script(db: &dyn DbLink, table: &str, event: &str, source: &str) -> DbResult<()> {
    store::ensure(db)?;
    db.execute(&format!(
        "INSERT INTO _phosphor_scripts(table_ref, event, source) VALUES ({}, {}, {}) \
         ON CONFLICT(table_ref, event) DO UPDATE SET source = {}",
        store::q(table),
        store::q(event),
        store::q(source),
        store::q(source)
    ))
    .map(|_| ())
}

pub fn clear_script(db: &dyn DbLink, table: &str, event: &str) -> DbResult<()> {
    db.execute(&format!(
        "DELETE FROM _phosphor_scripts WHERE table_ref = {} AND event = {}",
        store::q(table),
        store::q(event)
    ))
    .map(|_| ())
}

/// Every script as `(table, event, source)`, table-ordered.
pub fn all_scripts(db: &dyn DbLink) -> Vec<(String, String, String)> {
    db.query("SELECT table_ref, event, source FROM _phosphor_scripts ORDER BY table_ref, event")
        .map(|q| {
            q.rows
                .into_iter()
                .filter_map(|r| match (r.first(), r.get(1), r.get(2)) {
                    (Some(PValue::Text(t)), Some(PValue::Text(e)), Some(PValue::Text(s))) => {
                        Some((t.clone(), e.clone(), s.clone()))
                    }
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

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

// ── form lifecycle scripts (rule 4) ──────────────────────────────────

/// What a form/field event script produced.
#[derive(Debug, Default)]
pub struct FormOutcome {
    /// `say(...)` lines, in order.
    pub messages: Vec<String>,
    /// Set by `error("...")`; a non-None value blocks the save.
    pub error: Option<String>,
}

/// Run a lifecycle script against a record's final field values.
///
/// Globals beyond the standard `query`/`execute`/`say`:
///   record    a table of column -> value (read AND write)
///   field     the field the user was on (or nil)
///   is_new    true while inserting a new record
///   get(c)    record[c]
///   set(c, v) record[c] = v
///   error(m)  block the save with message m
///
/// `values` is updated in place with anything the script `set`.
pub fn run_form_event(
    db: &dyn DbLink,
    source: &str,
    values: &mut [(String, PValue)],
    field: Option<&str>,
    inserting: bool,
) -> Result<FormOutcome, String> {
    let lua = Lua::new();
    let _ = lua.set_memory_limit(32 * 1024 * 1024);
    lua.set_hook(
        mlua::HookTriggers::new().every_nth_instruction(200_000),
        |_lua, _debug| Err(mlua::Error::RuntimeError("script exceeded budget".into())),
    );

    let messages: RefCell<Vec<String>> = RefCell::new(Vec::new());
    let failure: RefCell<Option<String>> = RefCell::new(None);

    let result: mlua::Result<()> = lua.scope(|scope| {
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
            messages.borrow_mut().push(lua_to_string(&msg));
            Ok(())
        })?;
        let error = scope.create_function(|_, msg: Value| {
            *failure.borrow_mut() = Some(lua_to_string(&msg));
            Ok(())
        })?;

        // The record table: read/write view of the field values.
        let record = lua.create_table()?;
        for (name, v) in values.iter() {
            record.set(name.as_str(), to_lua(&lua, v)?)?;
        }
        let get = scope.create_function({
            let record = record.clone();
            move |_, name: String| record.get::<Value>(name.as_str())
        })?;
        let set = scope.create_function({
            let record = record.clone();
            move |lua, (name, v): (String, Value)| {
                record.set(name, v)?;
                let _ = lua;
                Ok(())
            }
        })?;

        let globals = lua.globals();
        globals.set("query", query)?;
        globals.set("execute", execute)?;
        globals.set("say", say)?;
        globals.set("error", error)?;
        globals.set("get", get)?;
        globals.set("set", set)?;
        globals.set("field", field.map(str::to_owned))?;
        globals.set("is_new", inserting)?;
        globals.set("record", record.clone())?;

        lua.load(source).exec()?;

        // Write the record table back into `values` (same columns only).
        for (name, v) in values.iter_mut() {
            let lv: Value = record.get(name.as_str())?;
            *v = from_lua(&lv);
        }
        Ok(())
    });

    if let Err(e) = result {
        return Err(e.to_string());
    }
    Ok(FormOutcome {
        messages: messages.into_inner(),
        error: failure.into_inner(),
    })
}

fn from_lua(v: &Value) -> PValue {
    match v {
        Value::Nil => PValue::Null,
        Value::Boolean(b) => PValue::Int(*b as i64),
        Value::Integer(i) => PValue::Int(*i),
        Value::Number(n) => PValue::Real(*n),
        Value::String(s) => PValue::Text(s.to_string_lossy().to_string()),
        other => PValue::Text(format!("{other:?}")),
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
    fn form_event_can_block_and_set() {
        let db = db();
        let mut values = vec![
            ("id".to_owned(), PValue::Int(1)),
            ("name".to_owned(), PValue::Text("  ada  ".to_owned())),
        ];
        // Block when empty, trim otherwise — one script, two behaviors.
        let src = r#"
            if record.name == nil or record.name == "" then
                error("name is required")
            else
                set("name", string.gsub(record.name, "^%s*(.-)%s*$", "%1"))
                say("trimmed to " .. record.name)
            end
        "#;
        let out = run_form_event(&db, src, &mut values, Some("name"), false).unwrap();
        assert!(out.error.is_none(), "{out:?}");
        assert_eq!(out.messages, vec!["trimmed to ada"]);
        assert_eq!(values[1].1, PValue::Text("ada".to_owned()));

        let mut empty = vec![("name".to_owned(), PValue::Null)];
        let out = run_form_event(&db, src, &mut empty, Some("name"), true).unwrap();
        assert_eq!(out.error.as_deref(), Some("name is required"));
    }

    #[test]
    fn runaway_loop_is_stopped() {
        let db = db();
        let err = run(&db, "while true do end").unwrap_err();
        assert!(err.contains("budget"), "{err}");
    }
}

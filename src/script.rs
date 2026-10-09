//! The scripting hook (DESIGN.md rules 1/2/3/5): a small, sandboxed Lua
//! environment that talks to the database only through [`DbLink`] and
//! exchanges the one [`PValue`] type — never a second conversion layer.
//!
//! Host API (shared by menu actions and form events):
//!
//!   query(sql)      rows as a sequence of column-keyed tables
//!   query_one(sql)  the first row (or nil)
//!   scalar(sql)     the first column of the first row (or nil)
//!   execute(sql)    affected-row count (-1 for a batch)
//!   exists(sql)     true when the query returns any row
//!   columns(table)  the table's column names (sequence)
//!   quote(s)        a single-quoted SQL literal
//!   ident(s)        a double-quoted SQL identifier
//!   say(v) / print(...)  append a line to the run's report
//!   trim(s) split(s, sep) join(list, sep) now() assert(cond, msg)
//!   json.encode(v) / json.decode(s)
//!
//! UI effects (rule 5) are queued, not performed, in a `ui` table:
//! `ui.refresh()`, `ui.browse(t)`, `ui.query(name)`, `ui.report(name)`,
//! `ui.form(t)`, `ui.prompt()`, `ui.quit()`. The app turns those into the
//! same `Command`s a keystroke would (rule 1), so a script inherits every
//! permission check — including `--readonly`.

use std::cell::RefCell;
use std::time::{Duration, Instant};

use mlua::{Lua, LuaOptions, StdLib, Value};

use crate::db::{DbLink, DbResult, PValue, QueryResult};
use crate::store;

/// A queued UI request from a script. Mapped 1:1 onto the command bus
/// by the App; never carried out by the sandbox itself.
#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    Refresh,
    Prompt,
    Browse(String),
    Query(String),
    Report(String),
    Form(String),
    Quit,
}

/// Everything a run produced.
#[derive(Debug, Default)]
pub struct Outcome {
    /// `say(...)`/`print(...)` lines, in order.
    pub messages: Vec<String>,
    /// Set by `error("...")`; a non-None value blocks a blocking event.
    pub error: Option<String>,
    /// Queued `ui.*` effects, in call order.
    pub effects: Vec<Effect>,
}

// ── script storage (`_phosphor_scripts`) ─────────────────────────────

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
    db.execute_params(
        "INSERT INTO _phosphor_scripts(table_ref, event, source) VALUES (?1, ?2, ?3) \
         ON CONFLICT(table_ref, event) DO UPDATE SET source = excluded.source",
        &[
            PValue::Text(table.into()),
            PValue::Text(event.into()),
            PValue::Text(source.into()),
        ],
    )
    .map(|_| ())
}

pub fn clear_script(db: &dyn DbLink, table: &str, event: &str) -> DbResult<()> {
    db.execute_params(
        "DELETE FROM _phosphor_scripts WHERE table_ref = ?1 AND event = ?2",
        &[PValue::Text(table.into()), PValue::Text(event.into())],
    )
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

// ── value marshalling ────────────────────────────────────────────────

/// BLOBs cross the boundary as tagged tables: `{__phosphor_blob, data}`.
/// mlua's `Buffer` is Luau-only (this build is Lua 5.4), so a raw byte
/// string cannot be told apart from text — an untagged string would be
/// lossily textified on the way back (#59).
fn blob_tag(lua: &Lua, b: &[u8]) -> mlua::Result<Value> {
    let t = lua.create_table()?;
    t.set("__phosphor_blob", true)?;
    t.set("data", lua.create_string(b)?)?;
    Ok(Value::Table(t))
}

/// The byte payload if `v` is a blob tag table, else `None`.
fn blob_payload(v: &Value) -> Option<Vec<u8>> {
    let Value::Table(t) = v else { return None };
    let tagged = t.raw_get::<bool>("__phosphor_blob").ok()?;
    if !tagged {
        return None;
    }
    let s: mlua::String = t.raw_get("data").ok()?;
    Some(s.as_bytes().to_vec())
}

fn to_lua(lua: &Lua, v: &PValue) -> mlua::Result<Value> {
    Ok(match v {
        PValue::Null => Value::Nil,
        PValue::Int(i) => Value::Integer(*i),
        PValue::Real(f) => Value::Number(*f),
        PValue::Text(t) => Value::String(lua.create_string(t)?),
        PValue::Blob(b) => blob_tag(lua, b)?,
    })
}

fn lua_to_string(v: &Value) -> String {
    if let Some(b) = blob_payload(v) {
        return PValue::blob_render(&b);
    }
    match v {
        Value::Nil => "nil".to_owned(),
        Value::Boolean(b) => b.to_string(),
        Value::Integer(i) => i.to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => s.to_string_lossy().to_string(),
        other => format!("{other:?}"),
    }
}

fn from_lua(v: &Value) -> Result<PValue, String> {
    if let Some(b) = blob_payload(v) {
        return Ok(PValue::Blob(b));
    }
    Ok(match v {
        Value::Nil => PValue::Null,
        Value::Boolean(b) => PValue::Int(*b as i64),
        Value::Integer(i) => PValue::Int(*i),
        Value::Number(n) => PValue::Real(*n),
        Value::String(s) => match std::str::from_utf8(s.as_bytes().as_ref()) {
            Ok(t) => PValue::Text(t.to_owned()),
            // Raw bytes from a script literal: keep them exactly rather
            // than lossily replacing with U+FFFD.
            Err(_) => PValue::Blob(s.as_bytes().to_vec()),
        },
        other => {
            return Err(format!(
                "cannot store a Lua {} in a cell: use a string, number, or nil",
                other.type_name()
            ))
        }
    })
}

/// One row as a table keyed by column name.
fn row_to_lua(lua: &Lua, columns: &[String], row: &[PValue]) -> mlua::Result<Value> {
    let t = lua.create_table()?;
    for (i, cell) in row.iter().enumerate() {
        if let Some(name) = columns.get(i) {
            t.set(name.as_str(), to_lua(lua, cell)?)?;
        }
    }
    Ok(Value::Table(t))
}

/// A query result as a sequence of row tables.
fn rows_to_lua(lua: &Lua, q: &QueryResult) -> mlua::Result<Value> {
    let t = lua.create_table()?;
    for (ri, row) in q.rows.iter().enumerate() {
        t.set(ri + 1, row_to_lua(lua, &q.columns, row)?)?;
    }
    Ok(Value::Table(t))
}

fn json_to_lua(lua: &Lua, j: &serde_json::Value) -> mlua::Result<Value> {
    use serde_json::Value as J;
    Ok(match j {
        J::Null => Value::Nil,
        J::Bool(b) => Value::Boolean(*b),
        J::Number(n) => match n.as_i64() {
            Some(i) => Value::Integer(i),
            None => Value::Number(n.as_f64().unwrap_or(0.0)),
        },
        J::String(s) => Value::String(lua.create_string(s)?),
        J::Array(a) => {
            let t = lua.create_table()?;
            for (i, x) in a.iter().enumerate() {
                t.set(i + 1, json_to_lua(lua, x)?)?;
            }
            Value::Table(t)
        }
        J::Object(o) => {
            let t = lua.create_table()?;
            for (k, v) in o {
                t.set(k.as_str(), json_to_lua(lua, v)?)?;
            }
            Value::Table(t)
        }
    })
}

fn lua_to_json(v: &Value) -> serde_json::Value {
    use serde_json::Value as J;
    if let Some(b) = blob_payload(v) {
        // Full hex: JSON consumers can round-trip it exactly.
        use std::fmt::Write as _;
        let mut hex = String::with_capacity(b.len() * 2);
        for x in &b {
            let _ = write!(hex, "{x:02x}");
        }
        return J::String(hex);
    }
    match v {
        Value::Nil => J::Null,
        Value::Boolean(b) => J::Bool(*b),
        Value::Integer(i) => J::Number((*i).into()),
        Value::Number(n) => serde_json::Number::from_f64(*n)
            .map(J::Number)
            .unwrap_or(J::Null),
        Value::String(s) => J::String(s.to_string_lossy().to_string()),
        Value::Table(t) => table_to_json(t),
        _ => J::Null,
    }
}

fn table_to_json(t: &mlua::Table) -> serde_json::Value {
    use serde_json::Value as J;
    // A sequence table becomes an array; anything else an object.
    let len = t.raw_len();
    if len > 0 {
        let mut arr = Vec::with_capacity(len);
        let mut seq = true;
        for i in 1..=len {
            match t.raw_get::<Value>(i) {
                Ok(Value::Nil) | Err(_) => {
                    seq = false;
                    break;
                }
                Ok(v) => arr.push(lua_to_json(&v)),
            }
        }
        if seq {
            return J::Array(arr);
        }
    }
    let mut map = serde_json::Map::new();
    for pair in t.clone().pairs::<Value, Value>().flatten() {
        map.insert(lua_to_string(&pair.0), lua_to_json(&pair.1));
    }
    J::Object(map)
}

fn sql_ident(s: &str) -> String {
    format!("\"{}\"", s.replace('"', "\"\""))
}

/// Stable saturating u64 → i64: `as i64` truncates the top bit and wraps
/// negative once seconds pass i64::MAX (≈ year 2262).
fn saturating_i64(secs: u64) -> i64 {
    if secs > i64::MAX as u64 {
        i64::MAX
    } else {
        secs as i64
    }
}

/// Seconds since the Unix epoch as an i64, without the u64→i64 truncation
/// that wraps negative at/after 2262, and without silently yielding 0 for a
/// pre-1970 clock (#79). Post-2262 saturates to i64::MAX; pre-1970 is a real
/// negative offset.
fn unix_secs(now: std::time::SystemTime) -> i64 {
    match now.duration_since(std::time::UNIX_EPOCH) {
        Ok(d) => saturating_i64(d.as_secs()),
        Err(_) => -saturating_i64(
            std::time::UNIX_EPOCH
                .duration_since(now)
                .map(|d| d.as_secs())
                .unwrap_or(0),
        ),
    }
}

/// A fresh sandbox with only in-memory libraries and the resource caps.
fn engine() -> mlua::Result<Lua> {
    // mlua's ALL_SAFE means safe for Rust, not isolated from the host:
    // it includes io, os, and package. Load only the libraries scripts
    // need for calculations and formatting. Coroutines are omitted
    // because the instruction hook below belongs to the main thread.
    let lua = Lua::new_with(
        StdLib::TABLE | StdLib::STRING | StdLib::UTF8 | StdLib::MATH,
        LuaOptions::default(),
    )?;
    // The base library is always loaded, including its file loaders.
    // Host code loads the authored script; scripts cannot load files
    // or dynamically supplied chunks themselves.
    for name in ["dofile", "loadfile", "load"] {
        lua.globals().set(name, Value::Nil)?;
    }
    // 32 MB of Lua heap is plenty for a menu action.
    lua.set_memory_limit(32 * 1024 * 1024)?;
    // 2M instructions: a full 10,000-row `query()` result with
    // realistic per-row work costs ~25-30 instructions/row (250k-300k),
    // so 200k killed legitimate scripts (#60). A runaway loop still
    // dies in milliseconds at 2M.
    const INSTRUCTION_BUDGET: u64 = 2_000_000;
    const HOOK_STEP: u32 = 200_000;
    let count = RefCell::new(0u64);
    lua.set_hook(
        mlua::HookTriggers::new().every_nth_instruction(HOOK_STEP),
        {
            let count = count.clone();
            move |_lua, _debug| {
                *count.borrow_mut() += u64::from(HOOK_STEP);
                if *count.borrow() >= INSTRUCTION_BUDGET {
                    Err(mlua::Error::RuntimeError(
                        "script exceeded its instruction budget".into(),
                    ))
                } else {
                    Ok(mlua::VmState::Continue)
                }
            }
        },
    );
    Ok(lua)
}

/// Per-run caps the instruction hook cannot see: time spent inside a
/// C callback (a DB round-trip) is not an instruction, so a loop of
/// 20,000 `query()` calls fit the instruction budget and could still
/// freeze the UI thread for minutes (#60).
const MAX_DB_CALLS: u32 = 1_000;
/// Wall time per script run, checked before each DB call. One call in
/// flight may still run to the transport timeout; everything after it
/// is refused.
const TIME_BUDGET: Duration = Duration::from_secs(5);

struct RunLimits {
    db_calls_left: u32,
    deadline: Instant,
}

impl RunLimits {
    fn new() -> Self {
        Self {
            db_calls_left: MAX_DB_CALLS,
            deadline: Instant::now() + TIME_BUDGET,
        }
    }

    /// Charge one DB round-trip; refuse when either cap is spent.
    fn take_db_call(&mut self) -> Result<(), String> {
        if self.db_calls_left == 0 {
            return Err(format!(
                "script exceeded its database budget ({MAX_DB_CALLS} calls)"
            ));
        }
        if Instant::now() > self.deadline {
            return Err("script exceeded its time budget".into());
        }
        self.db_calls_left -= 1;
        Ok(())
    }
}

/// Register the queued `ui` table inside a `lua.scope` block. A macro
/// (not a function) so the scope's inferred lifetimes flow through.
macro_rules! install_ui {
    ($scope:expr, $lua:expr, $effects:expr) => {{
        let effects: &RefCell<Vec<Effect>> = $effects;
        let ui = $lua.create_table()?;
        ui.set(
            "refresh",
            $scope.create_function(move |_, ()| {
                effects.borrow_mut().push(Effect::Refresh);
                Ok(())
            })?,
        )?;
        ui.set(
            "prompt",
            $scope.create_function(move |_, ()| {
                effects.borrow_mut().push(Effect::Prompt);
                Ok(())
            })?,
        )?;
        ui.set(
            "quit",
            $scope.create_function(move |_, ()| {
                effects.borrow_mut().push(Effect::Quit);
                Ok(())
            })?,
        )?;
        ui.set(
            "browse",
            $scope.create_function(move |_, arg: String| {
                effects.borrow_mut().push(Effect::Browse(arg));
                Ok(())
            })?,
        )?;
        ui.set(
            "query",
            $scope.create_function(move |_, arg: String| {
                effects.borrow_mut().push(Effect::Query(arg));
                Ok(())
            })?,
        )?;
        ui.set(
            "report",
            $scope.create_function(move |_, arg: String| {
                effects.borrow_mut().push(Effect::Report(arg));
                Ok(())
            })?,
        )?;
        ui.set(
            "form",
            $scope.create_function(move |_, arg: String| {
                effects.borrow_mut().push(Effect::Form(arg));
                Ok(())
            })?,
        )?;
        $lua.globals().set("ui", ui)?;
    }};
}

/// Register the shared data/helper globals. `$messages` receives output;
/// `$effects` receives `ui.*`.
macro_rules! install_host {
    ($scope:expr, $lua:expr, $db:expr, $messages:expr, $effects:expr, $limits:expr) => {{
        // Bind references outside the `move` closures (see install_ui).
        let messages: &RefCell<Vec<String>> = $messages;
        let db: &dyn DbLink = $db;
        let limits: &RefCell<RunLimits> = $limits;

        $lua.globals().set(
            "query",
            $scope.create_function(|lua, sql: String| {
                limits
                    .borrow_mut()
                    .take_db_call()
                    .map_err(mlua::Error::RuntimeError)?;
                let q = db.query(&sql).map_err(mlua::Error::RuntimeError)?;
                rows_to_lua(lua, &q)
            })?,
        )?;
        $lua.globals().set(
            "query_one",
            $scope.create_function(|lua, sql: String| {
                limits
                    .borrow_mut()
                    .take_db_call()
                    .map_err(mlua::Error::RuntimeError)?;
                let q = db.query(&sql).map_err(mlua::Error::RuntimeError)?;
                match q.rows.first() {
                    Some(row) => row_to_lua(lua, &q.columns, row),
                    None => Ok(Value::Nil),
                }
            })?,
        )?;
        $lua.globals().set(
            "scalar",
            $scope.create_function(|lua, sql: String| {
                limits
                    .borrow_mut()
                    .take_db_call()
                    .map_err(mlua::Error::RuntimeError)?;
                let q = db.query(&sql).map_err(mlua::Error::RuntimeError)?;
                match q.rows.first().and_then(|r| r.first()) {
                    Some(v) => to_lua(lua, v),
                    None => Ok(Value::Nil),
                }
            })?,
        )?;
        $lua.globals().set(
            "execute",
            $scope.create_function(|_, sql: String| {
                limits
                    .borrow_mut()
                    .take_db_call()
                    .map_err(mlua::Error::RuntimeError)?;
                let (n, _) = db.execute(&sql).map_err(mlua::Error::RuntimeError)?;
                Ok(n)
            })?,
        )?;
        $lua.globals().set(
            "exists",
            $scope.create_function(|_, sql: String| {
                limits
                    .borrow_mut()
                    .take_db_call()
                    .map_err(mlua::Error::RuntimeError)?;
                let q = db.query(&sql).map_err(mlua::Error::RuntimeError)?;
                Ok(!q.rows.is_empty())
            })?,
        )?;
        $lua.globals().set(
            "columns",
            $scope.create_function(|lua, table: String| {
                limits
                    .borrow_mut()
                    .take_db_call()
                    .map_err(mlua::Error::RuntimeError)?;
                let q = db
                    .query(&format!(
                        "SELECT name FROM pragma_table_info({})",
                        store::q(&table)
                    ))
                    .map_err(mlua::Error::RuntimeError)?;
                let t = lua.create_table()?;
                for (i, r) in q.rows.iter().enumerate() {
                    if let Some(PValue::Text(n)) = r.first() {
                        t.set(i + 1, n.as_str())?;
                    }
                }
                Ok(t)
            })?,
        )?;
        let say = $scope.create_function(|_, v: Value| {
            messages.borrow_mut().push(lua_to_string(&v));
            Ok(())
        })?;
        $lua.globals().set("say", say.clone())?;
        $lua.globals().set(
            "print",
            $scope.create_function(|_, args: mlua::Variadic<Value>| {
                let line = args.iter().map(lua_to_string).collect::<Vec<_>>().join(" ");
                messages.borrow_mut().push(line);
                Ok(())
            })?,
        )?;

        $lua.globals().set(
            "quote",
            $scope.create_function(|_, s: String| Ok(store::q(&s)))?,
        )?;
        $lua.globals().set(
            "ident",
            $scope.create_function(|_, s: String| Ok(sql_ident(&s)))?,
        )?;
        $lua.globals().set(
            "trim",
            $scope.create_function(|_, s: String| Ok(s.trim().to_owned()))?,
        )?;
        $lua.globals().set(
            "split",
            $scope.create_function(|lua, (s, sep): (String, String)| {
                let t = lua.create_table()?;
                let parts: Vec<&str> = if sep.is_empty() {
                    s.split_whitespace().collect()
                } else {
                    s.split(sep.as_str()).collect()
                };
                for (i, p) in parts.iter().enumerate() {
                    t.set(i + 1, *p)?;
                }
                Ok(t)
            })?,
        )?;
        $lua.globals().set(
            "join",
            $scope.create_function(|_, (list, sep): (mlua::Table, String)| {
                let mut out: Vec<String> = Vec::new();
                for v in list.sequence_values::<Value>() {
                    out.push(lua_to_string(&v?));
                }
                Ok(out.join(&sep))
            })?,
        )?;
        $lua.globals().set(
            "now",
            $scope.create_function(|_, ()| Ok(unix_secs(std::time::SystemTime::now())))?,
        )?;
        $lua.globals().set(
            "assert",
            $scope.create_function(|_, (cond, msg): (Value, Option<String>)| {
                let truthy = !matches!(cond, Value::Nil | Value::Boolean(false));
                if truthy {
                    Ok(())
                } else {
                    Err(mlua::Error::RuntimeError(
                        msg.unwrap_or_else(|| "assertion failed".to_owned()),
                    ))
                }
            })?,
        )?;

        let json = $lua.create_table()?;
        json.set(
            "encode",
            $scope.create_function(|_, v: Value| Ok(lua_to_json(&v).to_string()))?,
        )?;
        json.set(
            "decode",
            $scope.create_function(|lua, s: String| {
                let j: serde_json::Value = serde_json::from_str(&s)
                    .map_err(|e| mlua::Error::RuntimeError(format!("json: {e}")))?;
                json_to_lua(lua, &j)
            })?,
        )?;
        $lua.globals().set("json", json)?;

        install_ui!($scope, $lua, $effects);
    }};
}

// ── the runners ──────────────────────────────────────────────────────

/// Run `source` as a menu/standalone action. Returns the transcript and
/// any queued effects.
pub fn run(db: &dyn DbLink, source: &str) -> Result<Outcome, String> {
    let lua = engine().map_err(|e| e.to_string())?;
    let messages: RefCell<Vec<String>> = RefCell::new(Vec::new());
    let effects: RefCell<Vec<Effect>> = RefCell::new(Vec::new());

    let limits = RefCell::new(RunLimits::new());
    let result: mlua::Result<()> = lua.scope(|scope| {
        install_host!(scope, lua, db, &messages, &effects, &limits);
        // A trailing expression is reported too, so `return #rows` works.
        let ret: Value = lua.load(source).eval()?;
        if !matches!(ret, Value::Nil) {
            messages.borrow_mut().push(lua_to_string(&ret));
        }
        Ok(())
    });

    finish(result, messages, effects, None)
}

/// Run a lifecycle script against a record's final field values.
///
/// Globals beyond the standard ones:
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
) -> Result<Outcome, String> {
    let lua = engine().map_err(|e| e.to_string())?;
    let messages: RefCell<Vec<String>> = RefCell::new(Vec::new());
    let effects: RefCell<Vec<Effect>> = RefCell::new(Vec::new());
    let failure: RefCell<Option<String>> = RefCell::new(None);

    let limits = RefCell::new(RunLimits::new());
    let result: mlua::Result<()> = lua.scope(|scope| {
        install_host!(scope, lua, db, &messages, &effects, &limits);
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
            move |_, (name, v): (String, Value)| record.set(name, v)
        })?;

        let globals = lua.globals();
        globals.set("error", error)?;
        globals.set("get", get)?;
        globals.set("set", set)?;
        globals.set("field", field.map(str::to_owned))?;
        globals.set("is_new", inserting)?;
        globals.set("record", record.clone())?;

        lua.load(source).exec()?;

        // Write the record table back into `values` (same columns only).
        // A table/function stored by the script is a hard error here —
        // it used to become a "table: 0x…" pointer string in the DB (#59).
        for (name, v) in values.iter_mut() {
            let lv: Value = record.get(name.as_str())?;
            *v = from_lua(&lv).map_err(mlua::Error::RuntimeError)?;
        }
        Ok(())
    });

    let failure = failure.into_inner();
    finish(result, messages, effects, failure)
}

/// Assemble the outcome, defaulting an empty transcript to `script: ok`.
fn finish(
    result: mlua::Result<()>,
    messages: RefCell<Vec<String>>,
    effects: RefCell<Vec<Effect>>,
    error: Option<String>,
) -> Result<Outcome, String> {
    let mut out = Outcome {
        messages: messages.into_inner(),
        error,
        effects: effects.into_inner(),
    };
    match result {
        Ok(()) => {
            if out.messages.is_empty() {
                out.messages.push("script: ok".to_owned());
            }
            Ok(out)
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

    /// #79: the epoch-seconds cast must not wrap negative at/after 2262 or
    /// silently yield 0 for a pre-1970 clock.
    #[test]
    fn unix_secs_handles_extreme_clocks() {
        use std::time::{Duration, UNIX_EPOCH};
        // Normal post-epoch time passes through unchanged.
        let t = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        assert_eq!(unix_secs(t), 1_700_000_000);
        // At the 2262 boundary (i64::MAX seconds, the largest time a
        // SystemTime can hold): returns i64::MAX, never wraps negative.
        // (Past i64::MAX is covered by saturating_i64_clamps_at_max.)
        let at_max = UNIX_EPOCH + Duration::from_secs(i64::MAX as u64);
        assert_eq!(unix_secs(at_max), i64::MAX);
        // Pre-1970 clock: a real negative offset, not a silent 0.
        let pre = UNIX_EPOCH - Duration::from_secs(100);
        assert_eq!(unix_secs(pre), -100);
    }

    /// #79: the saturating cast itself must not wrap for huge inputs.
    #[test]
    fn saturating_i64_clamps_at_max() {
        assert_eq!(saturating_i64(0), 0);
        assert_eq!(saturating_i64(i64::MAX as u64), i64::MAX);
        // Past i64::MAX: clamp, never wrap negative.
        assert_eq!(saturating_i64(i64::MAX as u64 + 1), i64::MAX);
        assert_eq!(saturating_i64(u64::MAX), i64::MAX);
    }

    /// #79: the Lua `now()` global returns real epoch seconds (the binding
    /// is wired to the saturating path), not a truncated or zero value.
    #[test]
    fn now_returns_epoch_seconds() {
        let db = db();
        let out = run(&db, "say(now())").unwrap();
        let n: i64 = out.messages.join("\n").trim().parse().unwrap();
        // Any modern system clock is well past 2020-01-01 (1577836800).
        assert!(
            n > 1_577_836_800,
            "now() should be post-2020 epoch seconds, got {n}"
        );
    }

    #[test]
    fn execute_then_query() {
        let db = db();
        let out = run(
            &db,
            r#"
            local n = execute("INSERT INTO t(name) VALUES ('ada'), ('grace')")
            say("inserted " .. n)
            local rows = query("SELECT name FROM t ORDER BY name")
            say("first " .. rows[1].name)
            "#,
        )
        .unwrap();
        let msg = out.messages.join("\n");
        assert!(msg.contains("inserted 2"), "{msg}");
        assert!(msg.contains("first ada"), "{msg}");
    }

    #[test]
    fn nil_and_numbers_round_trip() {
        let db = db();
        db.execute("INSERT INTO t(name) VALUES (NULL)").unwrap();
        let out = run(
            &db,
            r#"
            local r = query("SELECT name FROM t")
            say(r[1].name == nil and "null" or "not-null")
            say(6 * 7)
            "#,
        )
        .unwrap();
        let msg = out.messages.join("\n");
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

    /// #60: a loop of DB round-trips is capped per run — 20,000
    /// `query()` calls fit the instruction budget (C-callback time is
    /// not an instruction) and could freeze the UI thread for minutes.
    #[test]
    fn db_call_loops_are_capped() {
        let db = db();
        let err = run(&db, "for i = 1, 20000 do query('SELECT 1') end").unwrap_err();
        assert!(err.contains("database budget"), "{err}");
    }

    /// #60: processing a full 10,000-row `query()` result with realistic
    /// per-row work (~25-30 instructions/row) fits the instruction
    /// budget — the old 200k cap died mid-run on exactly this.
    #[test]
    fn ten_k_row_scripts_fit_the_instruction_budget() {
        let db = db();
        db.execute("CREATE TABLE big(id INTEGER PRIMARY KEY, v TEXT)")
            .unwrap();
        db.execute(
            "INSERT INTO big(id, v) WITH RECURSIVE cnt(x) AS \
             (SELECT 1 UNION ALL SELECT x + 1 FROM cnt WHERE x < 10000) \
             SELECT x, 'v' || x FROM cnt",
        )
        .unwrap();
        let out = run(
            &db,
            r#"
            local rows = query("SELECT id, v FROM big ORDER BY id")
            local n = 0
            for i = 1, #rows do
              local r = rows[i]
              if r.id % 2 == 0 and r.v ~= nil and tostring(r.id) ~= "" then
                n = n + 1
              end
            end
            say(n)
            "#,
        )
        .unwrap();
        assert!(out.messages.join("\n").contains("5000"), "{out:?}");
    }

    #[test]
    fn sandbox_excludes_host_and_loading_capabilities_in_both_runners() {
        let db = db();
        let source = r#"
            for _, name in ipairs({
                "io", "os", "package", "require", "debug", "ffi",
                "loadfile", "dofile", "load", "coroutine"
            }) do
                assert(_G[name] == nil, name .. " must not be available")
            end
            local values = {string.upper(trim("  ada  ")), "Grace"}
            table.sort(values)
            assert(math.floor(4.5) == 4)
            assert(utf8.len("café") == 4)
            assert(json.decode(json.encode(values))[1] == "ADA")
            assert(scalar("SELECT 6 * 7") == 42)
            ui.browse("t")
            say(table.concat(values, ", "))
        "#;
        let menu = run(&db, source).unwrap();
        assert_eq!(menu.messages, ["ADA, Grace"]);
        assert_eq!(menu.effects, [Effect::Browse("t".into())]);

        let mut record = vec![("name".into(), PValue::Text("Ada".into()))];
        let form = run_form_event(&db, source, &mut record, Some("name"), true).unwrap();
        assert_eq!(form.messages, menu.messages);
        assert_eq!(form.effects, menu.effects);
        assert!(form.error.is_none());
    }

    #[test]
    fn sandbox_rejects_file_writes_and_file_loading() {
        let db = db();
        let path = std::env::temp_dir().join(format!(
            "phosphor-script-sandbox-{}.lua",
            std::process::id()
        ));
        std::fs::write(&path, "return 'host-file-executed'").unwrap();
        let quoted = serde_json::to_string(path.to_str().unwrap()).unwrap();
        let attempts = [
            format!("local f = io.open({quoted}, 'w'); f:write('changed'); f:close()"),
            format!("return dofile({quoted})"),
            format!("return loadfile({quoted})()"),
        ];
        for source in attempts {
            let result = run(&db, &source);
            let contents = std::fs::read_to_string(&path).unwrap();
            assert!(result.is_err(), "unexpected host access: {source}");
            assert_eq!(contents, "return 'host-file-executed'");
        }
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn ui_effects_queue_in_order() {
        let db = db();
        let out = run(
            &db,
            r#"
            ui.refresh()
            ui.browse("customers")
            ui.query("debtors")
            ui.report("orders")
            ui.form("customers")
            ui.prompt()
            ui.quit()
            "#,
        )
        .unwrap();
        assert_eq!(
            out.effects,
            vec![
                Effect::Refresh,
                Effect::Browse("customers".into()),
                Effect::Query("debtors".into()),
                Effect::Report("orders".into()),
                Effect::Form("customers".into()),
                Effect::Prompt,
                Effect::Quit,
            ]
        );
    }

    #[test]
    fn host_helpers() {
        let db = db();
        db.execute("INSERT INTO t(name) VALUES ('ada'), ('grace')")
            .unwrap();
        let out = run(
            &db,
            r#"
            say(scalar("SELECT count(*) FROM t"))
            say(query_one("SELECT name FROM t ORDER BY name").name)
            say(exists("SELECT 1 FROM t WHERE name = 'ada'") and "yes" or "no")
            say(trim("  hi  "))
            say(join(split("a,b,c", ","), "-"))
            say(quote("O'Brien"))
            say(ident("odd name"))
            assert(scalar("SELECT count(*) FROM t") == 2, "count wrong")
            say(columns("t")[2])
            "#,
        )
        .unwrap();
        let msg = out.messages.join("\n");
        assert!(msg.contains("\n2\n") || msg.starts_with('2'), "{msg}");
        assert!(msg.contains("ada"), "{msg}");
        assert!(msg.contains("yes"), "{msg}");
        assert!(msg.contains("hi"), "{msg}");
        assert!(msg.contains("a-b-c"), "{msg}");
        assert!(msg.contains("'O''Brien'"), "{msg}");
        assert!(msg.contains("\"odd name\""), "{msg}");
        assert!(msg.contains("name"), "{msg}");
    }

    #[test]
    fn json_round_trip() {
        let db = db();
        let out = run(
            &db,
            r#"
            local v = json.decode('{"a":[1,2,3],"ok":true}')
            say(v.a[2])
            say(v.ok and "true" or "false")
            say(json.encode({1, 2, 3}))
            say(json.encode({name = "ada", n = 3}))
            "#,
        )
        .unwrap();
        let msg = out.messages.join("\n");
        assert!(msg.contains("2"), "{msg}");
        assert!(msg.contains("true"), "{msg}");
        assert!(msg.contains("[1,2,3]"), "{msg}");
        assert!(msg.contains("\"name\":\"ada\""), "{msg}");
    }

    #[test]
    fn assert_blocks_on_false() {
        let db = db();
        let err = run(&db, r#"assert(1 == 2, "nope")"#).unwrap_err();
        assert!(err.contains("nope"), "{err}");
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
                set("name", trim(record.name))
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

    /// #59: BLOBs round-trip losslessly through a form event. They cross
    /// the boundary as tagged tables (mlua's Buffer is Luau-only) and are
    /// never downgraded to U+FFFD text — an echo script must come back
    /// byte-for-byte, storage class intact.
    #[test]
    fn blobs_round_trip_through_form_events() {
        let bytes: Vec<u8> = vec![0xff, 0xfe, 0x00, 0x10, b'x', b'y', b'z', b'w', b'!'];
        let mut values = vec![("data".to_owned(), PValue::Blob(bytes.clone()))];
        let out =
            run_form_event(&db(), "set('data', get('data'))", &mut values, None, false).unwrap();
        assert!(out.error.is_none(), "{out:?}");
        assert_eq!(values[0].1, PValue::Blob(bytes));
    }

    /// #59: storing a table or function in a cell is a hard error — it
    /// used to silently write a "table: 0x…" pointer string into the
    /// database, different garbage on every run.
    #[test]
    fn tables_and_functions_cannot_be_stored_in_cells() {
        let mut values = vec![("name".to_owned(), PValue::Text("a".to_owned()))];
        let err =
            run_form_event(&db(), "set('name', {1, 2, 3})", &mut values, None, false).unwrap_err();
        assert!(err.contains("table"), "{err}");
        assert_eq!(values[0].1, PValue::Text("a".to_owned()));

        let mut fnv = vec![("name".to_owned(), PValue::Text("a".to_owned()))];
        let err = run_form_event(
            &db(),
            "set('name', function() return 1 end)",
            &mut fnv,
            None,
            false,
        )
        .unwrap_err();
        assert!(err.contains("function"), "{err}");
    }
}

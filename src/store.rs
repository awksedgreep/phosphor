//! `_phosphor_*` tables: designer output stored IN the database
//! (DESIGN.md "Apps live in the database"). Copy the file, you copied
//! the application; replicate it, you deployed it.
//!
//! Everything goes through DbLink with literal SQL (single-quote
//! escaped), so storage works identically over a file and over sqld.

use crate::db::{DbLink, DbResult, PValue};

pub const DDL: &str = "
CREATE TABLE IF NOT EXISTS _phosphor_queries (
  id INTEGER PRIMARY KEY, name TEXT UNIQUE NOT NULL,
  table_ref TEXT, qbe_json TEXT, sql_text TEXT NOT NULL, version INTEGER DEFAULT 1);
CREATE TABLE IF NOT EXISTS _phosphor_reports (
  id INTEGER PRIMARY KEY, name TEXT UNIQUE NOT NULL,
  title TEXT, source_sql TEXT NOT NULL, group_by TEXT, version INTEGER DEFAULT 1);
CREATE TABLE IF NOT EXISTS _phosphor_forms (
  id INTEGER PRIMARY KEY, table_ref TEXT UNIQUE NOT NULL,
  layout_json TEXT NOT NULL, version INTEGER DEFAULT 1);
CREATE TABLE IF NOT EXISTS _phosphor_apps (
  id INTEGER PRIMARY KEY, name TEXT UNIQUE NOT NULL, description TEXT);
CREATE TABLE IF NOT EXISTS _phosphor_items (
  id INTEGER PRIMARY KEY, app_id INTEGER NOT NULL,
  label TEXT NOT NULL, action_kind TEXT NOT NULL, action_ref TEXT,
  hotkey TEXT, seq INTEGER DEFAULT 0);
";

pub fn ensure(db: &dyn DbLink) -> DbResult<()> {
    db.execute(DDL).map(|_| ())
}

/// SQL single-quote escaping for literal embedding (DbLink::query has no
/// bind parameters by design — it is the ad-hoc path).
pub fn q(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

pub fn text(v: Option<&PValue>) -> String {
    match v {
        Some(PValue::Text(t)) => t.clone(),
        Some(PValue::Null) | None => String::new(),
        Some(other) => other.render(),
    }
}

pub fn int(v: Option<&PValue>) -> i64 {
    match v {
        Some(PValue::Int(i)) => *i,
        _ => 0,
    }
}

/// Upsert by unique column; returns nothing — callers re-list.
pub fn upsert(
    db: &dyn DbLink,
    table: &str,
    key_col: &str,
    key: &str,
    cols: &[(&str, String)],
) -> DbResult<()> {
    ensure(db)?;
    let mut names: Vec<&str> = vec![key_col];
    let mut vals: Vec<String> = vec![q(key)];
    for (c, v) in cols {
        names.push(c);
        vals.push(q(v));
    }
    let sets: Vec<String> = cols
        .iter()
        .map(|(c, v)| format!("{c} = {}", q(v)))
        .collect();
    db.execute(&format!(
        "INSERT INTO {table} ({}) VALUES ({}) \
         ON CONFLICT({key_col}) DO UPDATE SET {}",
        names.join(", "),
        vals.join(", "),
        sets.join(", ")
    ))
    .map(|_| ())
}

pub fn lookup(
    db: &dyn DbLink,
    table: &str,
    key_col: &str,
    key: &str,
    want: &[&str],
) -> Option<Vec<String>> {
    ensure(db).ok()?;
    let sql = format!(
        "SELECT {} FROM {table} WHERE {key_col} = {} LIMIT 1",
        want.join(", "),
        q(key)
    );
    let out = db.query(&sql).ok()?;
    let row = out.rows.into_iter().next()?;
    Some(row.iter().map(|v| text(Some(v))).collect())
}

pub fn names(db: &dyn DbLink, table: &str, name_col: &str) -> Vec<String> {
    if ensure(db).is_err() {
        return Vec::new();
    }
    db.query(&format!("SELECT {name_col} FROM {table} ORDER BY {name_col}"))
        .map(|out| {
            out.rows
                .into_iter()
                .map(|r| text(r.first()))
                .collect()
        })
        .unwrap_or_default()
}

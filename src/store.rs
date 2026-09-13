//! `_phosphor_*` tables: designer output stored IN the database
//! (DESIGN.md "Apps live in the database"). Copy the file, you copied
//! the application; replicate it, you deployed it.
//!
//! Everything goes through DbLink with literal SQL (single-quote
//! escaped), so storage works identically over a file and over sqld.

use crate::db::{DbLink, DbResult, PValue};

#[derive(Clone, Copy, PartialEq)]
pub enum DesignSave {
    Save,
    SaveAs,
    Rename,
}

impl DesignSave {
    pub fn label(self) -> &'static str {
        match self {
            Self::Save => "save as",
            Self::SaveAs => "save as (keep original)",
            Self::Rename => "rename (move original)",
        }
    }
}

/// Named designer writes never silently replace another design. Renames
/// update application-menu references in the same savepoint, including
/// when the caller already owns a transaction.
pub fn save_design(
    db: &dyn DbLink,
    table: &str,
    kind: &str,
    name: &str,
    previous: Option<&str>,
    action: DesignSave,
    cols: &[(&str, String)],
) -> DbResult<()> {
    if name.trim().is_empty() {
        return Err("a design name is required".into());
    }
    ensure(db)?;
    db.execute("SAVEPOINT phosphor_design_save")?;
    let result = (|| {
        let update = previous.filter(|_| action != DesignSave::SaveAs);
        let mut values = vec![PValue::Text(name.into())];
        values.extend(cols.iter().map(|(_, v)| PValue::Text(v.clone())));
        let sql = if let Some(old) = update {
            let mut sets = vec!["name = ?1".to_owned()];
            sets.extend(
                cols.iter()
                    .enumerate()
                    .map(|(i, (c, _))| format!("{c} = ?{}", i + 2)),
            );
            values.push(PValue::Text(old.into()));
            format!(
                "UPDATE {table} SET {} WHERE name = ?{}",
                sets.join(", "),
                values.len()
            )
        } else {
            let mut names = vec!["name"];
            names.extend(cols.iter().map(|(c, _)| *c));
            format!(
                "INSERT INTO {table} ({}) VALUES ({})",
                names.join(", "),
                (1..=values.len())
                    .map(|i| format!("?{i}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        };
        let (changed, _) = db.execute_params(&sql, &values).map_err(|e| {
            if e.contains("UNIQUE constraint failed") {
                format!("{kind} {name:?} already exists; choose another name or reopen it and use F6 Save")
            } else { e }
        })?;
        if changed != 1 {
            return Err("the saved design no longer exists; use F7 Save As to create it".into());
        }
        if let Some(old) = update.filter(|old| *old != name) {
            db.execute_params(
                "UPDATE _phosphor_items SET action_ref = ?1 WHERE action_kind = ?2 AND action_ref = ?3",
                &[PValue::Text(name.into()), PValue::Text(kind.into()), PValue::Text(old.into())],
            )?;
        }
        Ok(())
    })();
    match result {
        Ok(()) => db.execute("RELEASE phosphor_design_save").map(|_| ()),
        Err(e) => {
            let rollback = db.execute("ROLLBACK TO phosphor_design_save");
            let release = db.execute("RELEASE phosphor_design_save");
            match (rollback, release) {
                (Ok(_), Ok(_)) => Err(e),
                (r, s) => Err(format!("{e}; restoring design failed: {r:?}; {s:?}")),
            }
        }
    }
}

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
  id INTEGER PRIMARY KEY, name TEXT UNIQUE NOT NULL, description TEXT,
  version INTEGER DEFAULT 1);
CREATE TABLE IF NOT EXISTS _phosphor_items (
  id INTEGER PRIMARY KEY, app_id INTEGER NOT NULL,
  label TEXT NOT NULL, action_kind TEXT NOT NULL, action_ref TEXT,
  hotkey TEXT, seq INTEGER DEFAULT 0);
CREATE TABLE IF NOT EXISTS _phosphor_prefs (
  user TEXT NOT NULL, key TEXT NOT NULL, value TEXT,
  UNIQUE(user, key));
-- Form/field lifecycle scripts (DESIGN.md rule 4): one Lua source per
-- (table, event). Authored with `script <table> <event> <lua>`.
CREATE TABLE IF NOT EXISTS _phosphor_scripts (
  id INTEGER PRIMARY KEY, table_ref TEXT NOT NULL, event TEXT NOT NULL,
  source TEXT NOT NULL, UNIQUE(table_ref, event));
CREATE INDEX IF NOT EXISTS idx_phosphor_items_app ON _phosphor_items(app_id);
";

/// Per-user preference (DESIGN.md `_phosphor_prefs`): a small
/// namespaced key/value store. `user` is fixed for now (single-user
/// desktop, 1988 spirit); the column is here for sqld multi-user later.
pub fn pref_set(db: &dyn DbLink, key: &str, value: &str) {
    if db.readonly() {
        return; // UI preferences still apply for this session.
    }
    // First preference write creates the table.
    ensure(db).ok();
    // Custom conflict target: UNIQUE(user, key), which the generic
    // upsert helper (single-column key) can't express.
    let _ = db.execute_params(
        "INSERT INTO _phosphor_prefs(user, key, value) VALUES ('me', ?1, ?2) \
         ON CONFLICT(user, key) DO UPDATE SET value = excluded.value",
        &[PValue::Text(key.into()), PValue::Text(value.into())],
    );
}

pub fn pref_get(db: &dyn DbLink, key: &str) -> Option<String> {
    lookup(db, "_phosphor_prefs", "key", key, &["value"])
        .map(|r| r.into_iter().next().unwrap_or_default())
        .filter(|v| !v.is_empty())
}

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
    let mut names: Vec<&str> = Vec::with_capacity(cols.len() + 1);
    let mut vals: Vec<PValue> = Vec::with_capacity(cols.len() + 1);
    names.push(key_col);
    vals.push(PValue::Text(key.into()));
    let mut sets: Vec<String> = Vec::with_capacity(cols.len());
    for (c, v) in cols {
        names.push(c);
        sets.push(format!("{c} = excluded.{c}"));
        vals.push(PValue::Text(v.clone()));
    }
    db.execute_params(
        &format!(
            "INSERT INTO {table} ({}) VALUES ({}) \
         ON CONFLICT({key_col}) DO UPDATE SET {}",
            names.join(", "),
            (1..=vals.len())
                .map(|i| format!("?{i}"))
                .collect::<Vec<_>>()
                .join(", "),
            sets.join(", ")
        ),
        &vals,
    )
    .map(|_| ())
}

/// READ path: never creates anything. A database without _phosphor
/// tables simply has no saved designs — that is not a reason to write
/// to it (found on camera: opening a designer sprouted five tables).
pub fn lookup(
    db: &dyn DbLink,
    table: &str,
    key_col: &str,
    key: &str,
    want: &[&str],
) -> Option<Vec<String>> {
    let sql = format!(
        "SELECT {} FROM {table} WHERE {key_col} = {} LIMIT 1",
        want.join(", "),
        q(key)
    );
    let out = db.query(&sql).ok()?;
    let row = out.rows.into_iter().next()?;
    Some(row.iter().map(|v| text(Some(v))).collect())
}

/// READ path: never creates anything (missing table = no names).
/// Capped at 1000 so a huge catalog can't full-scan+sort+transfer
/// into the UI on every designer open.
pub fn names(db: &dyn DbLink, table: &str, name_col: &str) -> Vec<String> {
    db.query(&format!(
        "SELECT {name_col} FROM {table} ORDER BY {name_col} LIMIT 1000"
    ))
    .map(|out| out.rows.into_iter().map(|r| text(r.first())).collect())
    .unwrap_or_default()
}

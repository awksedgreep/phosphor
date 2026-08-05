//! DbLink — the ONLY door to data (DESIGN.md, scripting-ready rule 2).
//!
//! Phase 1 ships the embedded backend (rusqlite, bundled SQLite). Phase 2
//! adds the sqld/Hrana backend behind the same trait; nothing above this
//! module may name rusqlite.

use std::time::{Duration, Instant};

use rusqlite::types::ValueRef;
use rusqlite::Connection;

pub type DbResult<T> = Result<T, String>;

/// The one value type that crosses every boundary (scripting-ready
/// rule 3): db rows, form fields, prompt results. Maps 1:1 onto SQLite
/// types, Hrana JSON, and (someday) Lua values.
#[derive(Debug, Clone, PartialEq)]
pub enum PValue {
    Null,
    Int(i64),
    Real(f64),
    Text(String),
    Blob(Vec<u8>),
}

impl PValue {
    fn from_ref(v: ValueRef<'_>) -> Self {
        match v {
            ValueRef::Null => PValue::Null,
            ValueRef::Integer(i) => PValue::Int(i),
            ValueRef::Real(f) => PValue::Real(f),
            ValueRef::Text(t) => PValue::Text(String::from_utf8_lossy(t).into_owned()),
            ValueRef::Blob(b) => PValue::Blob(b.to_vec()),
        }
    }

    /// One-line rendering for grid cells and forms.
    pub fn render(&self) -> String {
        match self {
            PValue::Null => "∅".into(),
            PValue::Int(i) => i.to_string(),
            PValue::Real(f) => {
                if f.fract() == 0.0 && f.abs() < 1e15 {
                    format!("{f:.1}")
                } else {
                    format!("{f}")
                }
            }
            PValue::Text(t) => t.replace('\n', "␤"),
            PValue::Blob(b) => {
                let head: String = b.iter().take(8).map(|x| format!("{x:02x}")).collect();
                let ell = if b.len() > 8 { "…" } else { "" };
                format!("x'{head}{ell}' ({}B)", b.len())
            }
        }
    }

    /// Parse an edited text back into a value, guided by the column's
    /// declared type. Empty input means NULL (dBASE would approve).
    pub fn parse(input: &str, decl_type: &str) -> PValue {
        if input.is_empty() {
            return PValue::Null;
        }
        let decl = decl_type.to_ascii_uppercase();
        if decl.contains("INT") {
            if let Ok(i) = input.parse::<i64>() {
                return PValue::Int(i);
            }
        }
        if decl.contains("REAL") || decl.contains("FLOA") || decl.contains("DOUB") {
            if let Ok(f) = input.parse::<f64>() {
                return PValue::Real(f);
            }
        }
        // NUMERIC affinity: numbers if they look like numbers.
        if decl.contains("NUM") || decl.contains("DEC") || decl.is_empty() {
            if let Ok(i) = input.parse::<i64>() {
                return PValue::Int(i);
            }
            if let Ok(f) = input.parse::<f64>() {
                return PValue::Real(f);
            }
        }
        PValue::Text(input.to_owned())
    }
}

impl rusqlite::ToSql for PValue {
    fn to_sql(&self) -> rusqlite::Result<rusqlite::types::ToSqlOutput<'_>> {
        use rusqlite::types::{ToSqlOutput, Value};
        Ok(match self {
            PValue::Null => ToSqlOutput::Owned(Value::Null),
            PValue::Int(i) => ToSqlOutput::Owned(Value::Integer(*i)),
            PValue::Real(f) => ToSqlOutput::Owned(Value::Real(*f)),
            PValue::Text(t) => ToSqlOutput::Borrowed(ValueRef::Text(t.as_bytes())),
            PValue::Blob(b) => ToSqlOutput::Borrowed(ValueRef::Blob(b)),
        })
    }
}

#[derive(Debug, Clone)]
pub struct TableInfo {
    pub name: String,
    pub is_view: bool,
}

#[derive(Debug, Clone)]
pub struct ColumnInfo {
    pub name: String,
    pub decl_type: String,
    pub notnull: bool,
    pub pk: bool,
}

#[derive(Debug, Clone)]
pub struct Page {
    pub rows: Vec<Vec<PValue>>,
    /// rowid per row when the table has one (enables EDIT).
    pub rowids: Option<Vec<i64>>,
}

#[derive(Debug, Clone)]
pub struct QueryResult {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<PValue>>,
    pub truncated: bool,
    pub elapsed: Duration,
}

/// Cap for ad-hoc prompt queries so a stray `SELECT * FROM million_rows`
/// stays interactive; the grid says so when it bites.
pub const QUERY_CAP: usize = 10_000;

pub trait DbLink {
    fn backend(&self) -> &'static str;
    fn name(&self) -> &str;
    fn tables(&self) -> DbResult<Vec<TableInfo>>;
    fn columns(&self, table: &str) -> DbResult<Vec<ColumnInfo>>;
    fn count(&self, table: &str) -> DbResult<i64>;
    /// True when the table has usable rowids (EDIT is possible).
    fn has_rowid(&self, table: &str) -> bool;
    fn page(&self, table: &str, offset: i64, limit: i64) -> DbResult<Page>;
    fn query(&self, sql: &str) -> DbResult<QueryResult>;
    /// Non-SELECT statement; returns affected-row count (-1 if unknown).
    fn execute(&self, sql: &str) -> DbResult<(i64, Duration)>;
    fn update_row(
        &self,
        table: &str,
        rowid: i64,
        changes: &[(String, PValue)],
    ) -> DbResult<()>;
    /// INSERT with the provided columns (omitted ones take DB defaults);
    /// returns the new rowid.
    fn insert_row(&self, table: &str, changes: &[(String, PValue)]) -> DbResult<i64>;
    fn delete_row(&self, table: &str, rowid: i64) -> DbResult<()>;
    /// Worst dbhealth_report status if the view exists and is readable
    /// ("ok" | "warn" | "attention" | "no data"), else None.
    fn health(&self) -> Option<String>;

    /// Tables whose declared foreign keys point AT `parent`:
    /// (child_table, child_col, parent_col). parent_col falls back to
    /// the parent's rowid pk when the FK names no column. Works on any
    /// backend — it's plain SQL over pragma table-functions.
    fn child_links(&self, parent: &str) -> Vec<(String, String, String)> {
        let Ok(tables) = self.tables() else { return Vec::new() };
        let mut out = Vec::new();
        for t in &tables {
            if t.name.eq_ignore_ascii_case(parent) {
                continue;
            }
            let sql = format!(
                "SELECT \"table\", \"from\", \"to\" FROM pragma_foreign_key_list({})",
                sql_str(&t.name)
            );
            let Ok(q) = self.query(&sql) else { continue };
            for row in &q.rows {
                let (PValue::Text(to_table), PValue::Text(from_col)) = (&row[0], &row[1])
                else {
                    continue;
                };
                if !to_table.eq_ignore_ascii_case(parent) {
                    continue;
                }
                let to_col = match &row[2] {
                    PValue::Text(c) => c.clone(),
                    _ => String::new(), // NULL → the parent's pk
                };
                out.push((t.name.clone(), from_col.clone(), to_col));
            }
        }
        out
    }
}

/// SQL string literal with quotes escaped.
fn sql_str(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

pub struct EmbeddedDb {
    conn: Connection,
    name: String,
}

impl EmbeddedDb {
    /// Open a database file. If PHOSPHOR_EXT names a loadable extension
    /// (e.g. libtimeless_ext.so), load it — capability, not dependency:
    /// failures are reported but the db still opens.
    pub fn open(path: &str) -> DbResult<(Self, Option<String>)> {
        let conn = Connection::open(path).map_err(|e| e.to_string())?;
        // Declared foreign keys should MEAN something: enforce them.
        // (SQLite defaults to off; existing orphan rows only surface
        // as errors on writes that would violate a constraint.)
        let _ = conn.execute_batch("PRAGMA foreign_keys = ON");
        let mut warning = None;
        if let Ok(ext) = std::env::var("PHOSPHOR_EXT") {
            if !ext.is_empty() {
                let loaded = unsafe {
                    let _guard = rusqlite::LoadExtensionGuard::new(&conn)
                        .map_err(|e| e.to_string())?;
                    conn.load_extension(&ext, None::<&str>)
                };
                if let Err(e) = loaded {
                    warning = Some(format!("PHOSPHOR_EXT not loaded: {e}"));
                }
            }
        }
        Ok((
            EmbeddedDb {
                conn,
                name: path.to_owned(),
            },
            warning,
        ))
    }

    fn quote(ident: &str) -> String {
        format!("\"{}\"", ident.replace('"', "\"\""))
    }

    fn collect_rows(
        stmt: &mut rusqlite::Statement<'_>,
        cap: usize,
    ) -> DbResult<(Vec<Vec<PValue>>, bool)> {
        let ncols = stmt.column_count();
        let mut rows_out = Vec::new();
        let mut truncated = false;
        let mut rows = stmt.query([]).map_err(|e| e.to_string())?;
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            if rows_out.len() >= cap {
                truncated = true;
                break;
            }
            let mut out = Vec::with_capacity(ncols);
            for i in 0..ncols {
                out.push(PValue::from_ref(
                    row.get_ref(i).map_err(|e| e.to_string())?,
                ));
            }
            rows_out.push(out);
        }
        Ok((rows_out, truncated))
    }
}

impl DbLink for EmbeddedDb {
    fn backend(&self) -> &'static str {
        "embedded"
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn tables(&self) -> DbResult<Vec<TableInfo>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT name, type FROM sqlite_master \
                 WHERE type IN ('table','view') AND name NOT LIKE 'sqlite_%' \
                 ORDER BY type = 'view', name",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |r| {
                Ok(TableInfo {
                    name: r.get(0)?,
                    is_view: r.get::<_, String>(1)? == "view",
                })
            })
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<_, _>>().map_err(|e| e.to_string())
    }

    fn columns(&self, table: &str) -> DbResult<Vec<ColumnInfo>> {
        let mut stmt = self
            .conn
            .prepare(&format!("PRAGMA table_info({})", Self::quote(table)))
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |r| {
                Ok(ColumnInfo {
                    name: r.get(1)?,
                    decl_type: r.get::<_, Option<String>>(2)?.unwrap_or_default(),
                    notnull: r.get::<_, i64>(3)? != 0,
                    pk: r.get::<_, i64>(5)? != 0,
                })
            })
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<_, _>>().map_err(|e| e.to_string())
    }

    fn count(&self, table: &str) -> DbResult<i64> {
        self.conn
            .query_row(
                &format!("SELECT count(*) FROM {}", Self::quote(table)),
                [],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())
    }

    fn has_rowid(&self, table: &str) -> bool {
        self.conn
            .prepare(&format!("SELECT rowid FROM {} LIMIT 0", Self::quote(table)))
            .is_ok()
    }

    fn page(&self, table: &str, offset: i64, limit: i64) -> DbResult<Page> {
        let q = Self::quote(table);
        if self.has_rowid(table) {
            let mut stmt = self
                .conn
                .prepare(&format!(
                    "SELECT rowid, * FROM {q} LIMIT {limit} OFFSET {offset}"
                ))
                .map_err(|e| e.to_string())?;
            let (mut rows, _) = Self::collect_rows(&mut stmt, limit as usize)?;
            let mut rowids = Vec::with_capacity(rows.len());
            for row in &mut rows {
                match row.remove(0) {
                    PValue::Int(id) => rowids.push(id),
                    _ => return Err("rowid was not an integer".into()),
                }
            }
            Ok(Page {
                rows,
                rowids: Some(rowids),
            })
        } else {
            let mut stmt = self
                .conn
                .prepare(&format!("SELECT * FROM {q} LIMIT {limit} OFFSET {offset}"))
                .map_err(|e| e.to_string())?;
            let (rows, _) = Self::collect_rows(&mut stmt, limit as usize)?;
            Ok(Page { rows, rowids: None })
        }
    }

    fn query(&self, sql: &str) -> DbResult<QueryResult> {
        let start = Instant::now();
        let mut stmt = self.conn.prepare(sql).map_err(|e| e.to_string())?;
        let columns: Vec<String> = stmt
            .column_names()
            .into_iter()
            .map(str::to_owned)
            .collect();
        let (rows, truncated) = Self::collect_rows(&mut stmt, QUERY_CAP)?;
        Ok(QueryResult {
            columns,
            rows,
            truncated,
            elapsed: start.elapsed(),
        })
    }

    fn execute(&self, sql: &str) -> DbResult<(i64, Duration)> {
        let start = Instant::now();
        // Multi-statement input goes through execute_batch (count
        // unknown): single-statement execute would prepare later
        // statements eagerly and fail name resolution against objects
        // the earlier statements create. A ';' inside a string literal
        // false-positives into batch — harmless, just loses the count.
        let body = sql.trim().trim_end_matches(';');
        if body.contains(';') {
            self.conn.execute_batch(sql).map_err(|e| e.to_string())?;
            return Ok((-1, start.elapsed()));
        }
        match self.conn.execute(sql, []) {
            Ok(n) => Ok((n as i64, start.elapsed())),
            Err(e) => Err(e.to_string()),
        }
    }

    fn update_row(
        &self,
        table: &str,
        rowid: i64,
        changes: &[(String, PValue)],
    ) -> DbResult<()> {
        if changes.is_empty() {
            return Ok(());
        }
        let sets: Vec<String> = changes
            .iter()
            .enumerate()
            .map(|(i, (col, _))| format!("{} = ?{}", Self::quote(col), i + 1))
            .collect();
        let sql = format!(
            "UPDATE {} SET {} WHERE rowid = ?{}",
            Self::quote(table),
            sets.join(", "),
            changes.len() + 1
        );
        let mut stmt = self.conn.prepare(&sql).map_err(|e| e.to_string())?;
        for (i, (_, v)) in changes.iter().enumerate() {
            stmt.raw_bind_parameter(i + 1, v).map_err(|e| e.to_string())?;
        }
        stmt.raw_bind_parameter(changes.len() + 1, rowid)
            .map_err(|e| e.to_string())?;
        let n = stmt.raw_execute().map_err(|e| e.to_string())?;
        if n == 1 {
            Ok(())
        } else {
            Err(format!("expected to update 1 row, updated {n}"))
        }
    }

    fn insert_row(&self, table: &str, changes: &[(String, PValue)]) -> DbResult<i64> {
        let sql = if changes.is_empty() {
            format!("INSERT INTO {} DEFAULT VALUES", Self::quote(table))
        } else {
            let cols: Vec<String> = changes.iter().map(|(c, _)| Self::quote(c)).collect();
            let marks: Vec<String> =
                (1..=changes.len()).map(|i| format!("?{i}")).collect();
            format!(
                "INSERT INTO {} ({}) VALUES ({})",
                Self::quote(table),
                cols.join(", "),
                marks.join(", ")
            )
        };
        let mut stmt = self.conn.prepare(&sql).map_err(|e| e.to_string())?;
        for (i, (_, v)) in changes.iter().enumerate() {
            stmt.raw_bind_parameter(i + 1, v).map_err(|e| e.to_string())?;
        }
        stmt.raw_execute().map_err(|e| e.to_string())?;
        Ok(self.conn.last_insert_rowid())
    }

    fn delete_row(&self, table: &str, rowid: i64) -> DbResult<()> {
        let n = self
            .conn
            .execute(
                &format!("DELETE FROM {} WHERE rowid = ?1", Self::quote(table)),
                [rowid],
            )
            .map_err(|e| e.to_string())?;
        if n == 1 {
            Ok(())
        } else {
            Err(format!("expected to delete 1 row, deleted {n}"))
        }
    }

    fn health(&self) -> Option<String> {
        // Worst-first ordering is part of the dbhealth_report contract.
        let exists: i64 = self
            .conn
            .query_row(
                "SELECT count(*) FROM sqlite_master \
                 WHERE type = 'view' AND name LIKE '%\\_report' ESCAPE '\\'",
                [],
                |r| r.get(0),
            )
            .ok()?;
        if exists == 0 {
            return None;
        }
        let view: String = self
            .conn
            .query_row(
                "SELECT name FROM sqlite_master \
                 WHERE type = 'view' AND name LIKE '%\\_report' ESCAPE '\\' \
                 ORDER BY name LIMIT 1",
                [],
                |r| r.get(0),
            )
            .ok()?;
        self.conn
            .query_row(
                &format!("SELECT status FROM {} LIMIT 1", Self::quote(&view)),
                [],
                |r| r.get(0),
            )
            .ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn testdb() -> EmbeddedDb {
        let (db, _) = EmbeddedDb::open(":memory:").unwrap();
        db.execute(
            "CREATE TABLE people(id INTEGER PRIMARY KEY, name TEXT, score REAL);
             INSERT INTO people(name, score) VALUES ('ada', 99.5), ('grace', 100.0);
             CREATE VIEW top AS SELECT name FROM people WHERE score > 99.9;",
        )
        .unwrap();
        db
    }

    #[test]
    fn tables_and_columns() {
        let db = testdb();
        let tables = db.tables().unwrap();
        let names: Vec<_> = tables.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, ["people", "top"]);
        assert!(tables[1].is_view);
        let cols = db.columns("people").unwrap();
        assert_eq!(cols.len(), 3);
        assert!(cols[0].pk);
        assert_eq!(cols[2].decl_type, "REAL");
    }

    #[test]
    fn paging_rowids_and_edit() {
        let db = testdb();
        assert_eq!(db.count("people").unwrap(), 2);
        assert!(db.has_rowid("people"));
        assert!(!db.has_rowid("top"));
        let page = db.page("people", 0, 10).unwrap();
        assert_eq!(page.rows.len(), 2);
        let rowids = page.rowids.unwrap();
        db.update_row(
            "people",
            rowids[0],
            &[("name".into(), PValue::Text("ada lovelace".into()))],
        )
        .unwrap();
        let q = db.query("SELECT name FROM people ORDER BY id").unwrap();
        assert_eq!(q.rows[0][0], PValue::Text("ada lovelace".into()));
    }

    #[test]
    fn query_caps_and_execute() {
        let db = testdb();
        let (n, _) = db
            .execute("INSERT INTO people(name, score) VALUES ('x', 1.0)")
            .unwrap();
        assert_eq!(n, 1);
        let q = db.query("SELECT * FROM people").unwrap();
        assert_eq!(q.columns, ["id", "name", "score"]);
        assert_eq!(q.rows.len(), 3);
        assert!(!q.truncated);
    }

    #[test]
    fn value_parse_respects_decl_type() {
        assert_eq!(PValue::parse("42", "INTEGER"), PValue::Int(42));
        assert_eq!(PValue::parse("4.5", "REAL"), PValue::Real(4.5));
        assert_eq!(PValue::parse("42", "TEXT"), PValue::Text("42".into()));
        assert_eq!(PValue::parse("", "TEXT"), PValue::Null);
    }
}

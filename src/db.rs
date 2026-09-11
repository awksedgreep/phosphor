//! DbLink — the ONLY door to data (DESIGN.md, scripting-ready rule 2).
//!
//! Phase 1 ships the embedded backend (rusqlite, bundled SQLite). Phase 2
//! adds the sqld/Hrana backend behind the same trait; nothing above this
//! module may name rusqlite.

use std::cell::RefCell;
use std::collections::HashMap;
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
                use std::fmt::Write as _;
                let mut head = String::with_capacity(16);
                for x in b.iter().take(8) {
                    let _ = write!(head, "{x:02x}");
                }
                let ell = if b.len() > 8 { "…" } else { "" };
                format!("x'{head}{ell}' ({}B)", b.len())
            }
        }
    }

    /// Case-insensitive substring match for `find` (ASCII folding —
    /// same semantics as the old render()+to_lowercase path, but with
    /// zero allocation on the common Text/Int/Real/Null cases).
    /// `needle_lc` must already be lowercased.
    pub fn contains_ci(&self, needle_lc: &str, needle_has_alpha: bool) -> bool {
        if needle_lc.is_empty() {
            return true;
        }
        match self {
            PValue::Text(t) => {
                let (h, n) = (t.as_bytes(), needle_lc.as_bytes());
                n.len() <= h.len()
                    && h.windows(n.len()).any(|w| w.eq_ignore_ascii_case(n))
            }
            // Numeric renders contain no letters: an alpha needle can
            // never match, so skip rendering entirely.
            PValue::Int(i) => !needle_has_alpha && i.to_string().contains(needle_lc),
            PValue::Real(_) => !needle_has_alpha && self.render().contains(needle_lc),
            PValue::Null => "∅".contains(needle_lc),
            // Rare + mixed-case render ("(5B)"): keep the slow path.
            PValue::Blob(_) => self.render().to_ascii_lowercase().contains(needle_lc),
        }
    }

    /// Length of render() in chars, WITHOUT building the String:
    /// grid width sampling calls this 50×cols per open.
    pub fn render_len(&self) -> usize {
        match self {
            PValue::Null => 1, // "∅"
            PValue::Int(mut i) => {
                if i == 0 {
                    return 1;
                }
                let mut n = 0;
                if i < 0 {
                    n += 1; // '-'
                    i = i.checked_abs().unwrap_or(i64::MAX);
                }
                while i > 0 {
                    n += 1;
                    i /= 10;
                }
                n
            }
            // replace('\n', "␤") is char-count neutral.
            PValue::Text(t) => t.chars().count(),
            // Formatting-dependent (e.g. integral reals gain ".0"):
            // fall back to rendering (rare in width sampling).
            _ => self.render().chars().count(),
        }
    }

    /// Parse an edited text back into a value, guided by the column's
    /// declared type. Empty input means NULL (dBASE would approve).
    pub fn parse(input: &str, decl_type: &str) -> PValue {
        if input.is_empty() {
            return PValue::Null;
        }
        // Case-insensitive substring checks without allocating an
        // uppercased copy per keystroke/edit.
        fn contains_ci(hay: &str, needle: &str) -> bool {
            if needle.is_empty() || hay.len() < needle.len() {
                return false;
            }
            hay.as_bytes()
                .windows(needle.len())
                .any(|w| w.eq_ignore_ascii_case(needle.as_bytes()))
        }
        if contains_ci(decl_type, "INT") {
            if let Ok(i) = input.parse::<i64>() {
                return PValue::Int(i);
            }
        }
        if contains_ci(decl_type, "REAL")
            || contains_ci(decl_type, "FLOA")
            || contains_ci(decl_type, "DOUB")
        {
            if let Ok(f) = input.parse::<f64>() {
                return PValue::Real(f);
            }
        }
        // NUMERIC affinity: numbers if they look like numbers.
        if contains_ci(decl_type, "NUM") || contains_ci(decl_type, "DEC") || decl_type.is_empty() {
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
    /// A window PLUS the table total. Default is page()+count() (two
    /// round-trips); backends collapse it into one query with
    /// count(*) OVER (). Used on open/refresh only — per-turn paging
    /// stays on page() (re-counting every PgDn would be worse).
    fn open_window(&self, table: &str, offset: i64, limit: i64) -> DbResult<(Page, i64)> {
        let page = self.page(table, offset, limit)?;
        let total = self.count(table)?;
        Ok((page, total))
    }
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
    rowid_cache: RefCell<HashMap<String, bool>>,
}

/// Split a `SELECT ..., count(*) OVER () AS _total` result back into a
/// Page + total: every row carries the same total in its trailing
/// column, rowid tables carry rowids in the leading column. Shared by
/// both backends' open_window. Errs (→ caller falls back to
/// page()+count()) on any structural surprise.
pub(crate) fn strip_window_total(
    mut rows: Vec<Vec<PValue>>,
    with_rowid: bool,
) -> DbResult<(Page, i64)> {
    if rows.is_empty() {
        return Err("empty window carries no total".into());
    }
    let total = match rows[0].pop() {
        Some(PValue::Int(t)) => t,
        _ => return Err("total column was not an integer".into()),
    };
    for row in rows.iter_mut().skip(1) {
        if !matches!(row.pop(), Some(PValue::Int(_))) {
            return Err("total column was not an integer".into());
        }
    }
    if with_rowid {
        let mut rowids = Vec::with_capacity(rows.len());
        for row in &mut rows {
            if row.is_empty() {
                return Err("rowid column missing".into());
            }
            match row.remove(0) {
                PValue::Int(id) => rowids.push(id),
                _ => return Err("rowid was not an integer".into()),
            }
        }
        Ok((Page { rows, rowids: Some(rowids) }, total))
    } else {
        Ok((Page { rows, rowids: None }, total))
    }
}

/// True when `sql` already constrains its row count (conservative
/// substring check — a false positive inside a string literal just
/// skips the optimization, never changes semantics).
pub(crate) fn sql_has_limit(sql: &str) -> bool {
    let lower = sql.to_ascii_lowercase();
    lower.contains("limit")
}

/// Append a server-side cap so a stray `SELECT * FROM million_rows`
/// doesn't plan/execute a full scan+sort client-truncated later.
/// Caller must pass QUERY_CAP+1 so `truncated` stays accurate.
pub(crate) fn apply_cap(sql: &str, cap_plus_one: usize) -> String {
    let t = sql.trim().trim_end_matches(';').trim_end();
    if sql_has_limit(t) {
        t.to_owned()
    } else {
        format!("{t} LIMIT {cap_plus_one}")
    }
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
                rowid_cache: RefCell::new(HashMap::new()),
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
            .prepare_cached(
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
            .prepare_cached(&format!("PRAGMA table_info({})", Self::quote(table)))
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
        if let Some(&known) = self.rowid_cache.borrow().get(table) {
            return known;
        }
        let ok = self
            .conn
            .prepare_cached(&format!("SELECT rowid FROM {} LIMIT 0", Self::quote(table)))
            .is_ok();
        self.rowid_cache.borrow_mut().insert(table.to_owned(), ok);
        ok
    }

    fn page(&self, table: &str, offset: i64, limit: i64) -> DbResult<Page> {
        let q = Self::quote(table);
        if self.has_rowid(table) {
            let mut stmt = self
                .conn
                .prepare_cached(&format!(
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
                .prepare_cached(&format!("SELECT * FROM {q} LIMIT {limit} OFFSET {offset}"))
                .map_err(|e| e.to_string())?;
            let (rows, _) = Self::collect_rows(&mut stmt, limit as usize)?;
            Ok(Page { rows, rowids: None })
        }
    }

    /// Single-query open_window: the total rides along as a trailing
    /// count(*) OVER () column, stripped back off here. Falls back to
    /// page()+count() on any structural surprise.
    fn open_window(&self, table: &str, offset: i64, limit: i64) -> DbResult<(Page, i64)> {
        let fallback = || {
            let page = self.page(table, offset, limit)?;
            let total = self.count(table)?;
            Ok((page, total))
        };
        let q = Self::quote(table);
        let with_rowid = self.has_rowid(table);
        let select = if with_rowid {
            format!("SELECT rowid, *, count(*) OVER () AS _total FROM {q}")
        } else {
            format!("SELECT *, count(*) OVER () AS _total FROM {q}")
        };
        let mut stmt = match self.conn.prepare_cached(&format!(
            "{select} LIMIT {limit} OFFSET {offset}"
        )) {
            Ok(s) => s,
            Err(_) => return fallback(),
        };
        let (rows, _) = match Self::collect_rows(&mut stmt, limit.max(0) as usize) {
            Ok(r) => r,
            Err(_) => return fallback(),
        };
        if rows.is_empty() {
            if offset == 0 {
                // Open on an empty table: total is exactly 0, no count needed.
                let page = Page { rows: Vec::new(), rowids: with_rowid.then(Vec::new) };
                return Ok((page, 0));
            }
            return fallback();
        }
        match strip_window_total(rows, with_rowid) {
            Ok(ok) => Ok(ok),
            Err(_) => fallback(),
        }
    }

    fn query(&self, sql: &str) -> DbResult<QueryResult> {
        let start = Instant::now();
        // Push the cap into SQLite so the engine can stop early instead
        // of planning/executing a full scan we truncate client-side.
        // QUERY_CAP+1 rows => truncated flag stays exact.
        let capped = apply_cap(sql, QUERY_CAP + 1);
        let mut stmt = self.conn.prepare(&capped).map_err(|e| e.to_string())?;
        let columns: Vec<String> = stmt
            .column_names()
            .into_iter()
            .map(str::to_owned)
            .collect();
        let (mut rows, _) = Self::collect_rows(&mut stmt, QUERY_CAP + 1)?;
        let truncated = rows.len() > QUERY_CAP;
        rows.truncate(QUERY_CAP);
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
        // DDL may change rowid-ness; drop cached probes (cheap: only on write/DDL path).
        let lower = sql.to_ascii_lowercase();
        if lower.contains("create")
            || lower.contains("drop")
            || lower.contains("alter")
            || lower.contains("vacuum")
        {
            self.rowid_cache.borrow_mut().clear();
        }
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
        let mut stmt = self.conn.prepare_cached(&sql).map_err(|e| e.to_string())?;
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
        let mut stmt = self.conn.prepare_cached(&sql).map_err(|e| e.to_string())?;
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
        // Single sqlite_master probe (was count(*) + pick = 2 trips).
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

    /// Batched override: one UNION ALL over pragma_foreign_key_list
    /// instead of the default method's N queries (one per table).
    /// Column order mirrors the default: (child_tbl, to_table, from, to).
    fn child_links(&self, parent: &str) -> Vec<(String, String, String)> {
        let Ok(tables) = self.tables() else { return Vec::new() };
        let parts: Vec<String> = tables
            .iter()
            .filter(|t| !t.name.eq_ignore_ascii_case(parent))
            .map(|t| {
                format!(
                    "SELECT {} AS child_tbl, \"table\" AS to_table, \
                     \"from\" AS from_col, \"to\" AS to_col \
                     FROM pragma_foreign_key_list({})",
                    sql_str(&t.name),
                    sql_str(&t.name)
                )
            })
            .collect();
        if parts.is_empty() {
            return Vec::new();
        }
        let Ok(q) = self.query(&parts.join(" UNION ALL ")) else { return Vec::new() };
        let mut out = Vec::new();
        for row in &q.rows {
            let [PValue::Text(child), PValue::Text(to_table), PValue::Text(from_col), to_col] =
                row.as_slice()
            else {
                continue;
            };
            if !to_table.eq_ignore_ascii_case(parent) {
                continue;
            }
            let to_col = match to_col {
                PValue::Text(c) => c.clone(),
                _ => String::new(), // NULL → the parent's pk
            };
            out.push((child.clone(), from_col.clone(), to_col));
        }
        out
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
    fn open_window_matches_page_plus_count() {
        let (db, _) = EmbeddedDb::open(":memory:").unwrap();
        db.execute(
            "CREATE TABLE t(id INTEGER PRIMARY KEY, v TEXT);
             INSERT INTO t(v) VALUES ('a'), ('b'), ('c');",
        )
        .unwrap();
        // Rowid table: window + total + rowids in one trip.
        let (page, total) = db.open_window("t", 1, 2).unwrap();
        assert_eq!(total, 3);
        assert_eq!(page.rows.len(), 2);
        assert_eq!(page.rowids, Some(vec![2, 3]));
        // Rowid-less view: no rowids, total still rides along.
        db.execute("CREATE VIEW v AS SELECT v AS letter FROM t").unwrap();
        let (page, total) = db.open_window("v", 0, 10).unwrap();
        assert_eq!((total, page.rows.len()), (3, 3));
        assert_eq!(page.rowids, None);
        // Empty table at offset 0: total exactly 0, no count query.
        db.execute("CREATE TABLE e(id INTEGER PRIMARY KEY)").unwrap();
        let (page, total) = db.open_window("e", 0, 10).unwrap();
        assert_eq!((total, page.rows.len()), (0, 0));
        // Past-the-end window falls back to count (total still right).
        let (_, total) = db.open_window("t", 99, 10).unwrap();
        assert_eq!(total, 3);
    }

    #[test]
    fn render_len_matches_render() {
        for v in [
            PValue::Null,
            PValue::Int(0),
            PValue::Int(42),
            PValue::Int(-987654321),
            PValue::Int(i64::MIN),
            PValue::Int(i64::MAX),
            PValue::Real(100.0),
            PValue::Real(4.5),
            PValue::Text("héllo\nworld".into()),
            PValue::Text(String::new()),
            PValue::Blob(vec![0xab, 0x12]),
            PValue::Blob(vec![0; 20]),
        ] {
            assert_eq!(v.render_len(), v.render().chars().count(), "{v:?}");
        }
    }

    #[test]
    fn contains_ci_matches_lowercased_render() {
        // New zero-alloc matcher must agree with the old
        // render().to_ascii_lowercase().contains() path.
        let cases = vec![
            PValue::Text("Ada Lovelace".into()),
            PValue::Text("héllo".into()),
            PValue::Int(42),
            PValue::Int(-7),
            PValue::Real(100.0),
            PValue::Real(4.5),
            PValue::Null,
            PValue::Blob(vec![0xab, 0x12]),
        ];
        for needle in ["ada", "LACE", "42", "100", "∅", "x'ab", "zzz", ""] {
            let lc = needle.to_ascii_lowercase();
            let has_alpha = lc.bytes().any(|b| b.is_ascii_alphabetic());
            for v in &cases {
                let old = v.render().to_ascii_lowercase().contains(&lc);
                assert_eq!(v.contains_ci(&lc, has_alpha), old, "{v:?} vs {needle:?}");
            }
        }
    }

    #[test]
    fn child_links_finds_fk_children() {
        let (db, _) = EmbeddedDb::open(":memory:").unwrap();
        db.execute(
            "CREATE TABLE customers(id INTEGER PRIMARY KEY, name TEXT);
             CREATE TABLE orders(id INTEGER PRIMARY KEY, customer_id INTEGER REFERENCES customers(id), total REAL);
             CREATE TABLE orphan(id INTEGER PRIMARY KEY, note TEXT);",
        )
        .unwrap();
        let mut links = db.child_links("customers");
        links.sort();
        assert_eq!(links, [("orders".to_owned(), "customer_id".to_owned(), "id".to_owned())]);
        assert!(db.child_links("orphan").is_empty());
    }

    #[test]
    fn value_parse_respects_decl_type() {
        assert_eq!(PValue::parse("42", "INTEGER"), PValue::Int(42));
        assert_eq!(PValue::parse("4.5", "REAL"), PValue::Real(4.5));
        assert_eq!(PValue::parse("42", "TEXT"), PValue::Text("42".into()));
        assert_eq!(PValue::parse("", "TEXT"), PValue::Null);
    }
}

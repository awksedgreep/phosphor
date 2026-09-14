//! DbLink — the ONLY door to data (DESIGN.md, scripting-ready rule 2).
//!
//! Phase 1 ships the embedded backend (rusqlite, bundled SQLite). Phase 2
//! adds the sqld/Hrana backend behind the same trait; nothing above this
//! module may name rusqlite.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use rusqlite::types::ValueRef;
use rusqlite::Connection;

pub type DbResult<T> = Result<T, String>;

pub enum QueryEvent {
    Columns(Vec<String>),
    Row(Vec<PValue>),
    End,
}

pub type RowSink = Box<dyn FnMut(QueryEvent) -> DbResult<()> + Send>;

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
    pub(crate) fn from_ref(v: ValueRef<'_>) -> Self {
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
                n.len() <= h.len() && h.windows(n.len()).any(|w| w.eq_ignore_ascii_case(n))
            }
            // Numeric renders contain no letters when finite: an alpha
            // needle can never match, so skip rendering entirely.
            PValue::Int(i) => !needle_has_alpha && i.to_string().contains(needle_lc),
            // NaN/inf render with letters — fall through to the slow path.
            PValue::Real(f) if f.is_finite() => {
                !needle_has_alpha && self.render().contains(needle_lc)
            }
            PValue::Null => "∅".contains(needle_lc),
            // Rare + lettered render (non-finite reals, blob hex):
            // keep the slow path.
            _ => self.render().to_ascii_lowercase().contains(needle_lc),
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

#[derive(Debug, Clone, PartialEq)]
pub struct TableInfo {
    pub name: String,
    pub is_view: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ColumnInfo {
    pub name: String,
    pub decl_type: String,
    pub notnull: bool,
    pub pk: bool,
    /// SQLite computes this column (VIRTUAL or STORED); never write it.
    pub generated: bool,
    /// Raw SQL text of the DEFAULT expression (e.g. `0`, `'abc'`),
    /// as stored by pragma table_info — None when the column has no
    /// default. Used by the TABLE EDITOR to round-trip constraints.
    pub dflt_value: Option<String>,
}

/// Match SELECT * order, including generated columns but excluding
/// hidden virtual-table arguments.
pub(crate) const COLUMN_INFO_SQL: &str =
    "SELECT * FROM pragma_table_xinfo(?1) WHERE hidden != 1 ORDER BY cid";

#[derive(Debug, Clone, PartialEq)]
pub struct Page {
    pub rows: Vec<Vec<PValue>>,
    /// rowid per row when the table has one (enables EDIT).
    pub rowids: Option<Vec<i64>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct QueryResult {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<PValue>>,
    pub truncated: bool,
    pub elapsed: Duration,
}

/// Cap for ad-hoc prompt queries so a stray `SELECT * FROM million_rows`
/// stays interactive; the grid says so when it bites.
pub const QUERY_CAP: usize = 10_000;

pub trait DbLink: Send {
    fn backend(&self) -> &'static str;
    fn name(&self) -> &str;
    fn readonly(&self) -> bool;
    fn save_scratch(&mut self, _path: &str) -> DbResult<()> {
        Err("Save Database is available for a writable scratch database only".into())
    }
    fn require_writable(&self) -> DbResult<()> {
        if self.readonly() {
            Err("read-only mode: database writes are disabled".into())
        } else {
            Ok(())
        }
    }
    fn tables(&self) -> DbResult<Vec<TableInfo>>;
    fn columns(&self, table: &str) -> DbResult<Vec<ColumnInfo>>;
    fn count(&self, table: &str) -> DbResult<i64>;
    /// An unshadowed SQLite rowid alias; None means BROWSE is read-only.
    fn rowid_column(&self, table: &str) -> DbResult<Option<String>>;
    fn has_rowid(&self, table: &str) -> bool {
        self.rowid_column(table).ok().flatten().is_some()
    }
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
    /// Read one complete SELECT without the interactive preview cap.
    /// End is delivered only after successful completion, including zero rows.
    fn stream_query(&self, sql: &str, sink: RowSink) -> DbResult<usize>;
    /// Layouts needing multiple passes retain the complete streamed result.
    fn query_complete(&self, sql: &str) -> DbResult<QueryResult> {
        let start = Instant::now();
        let data = std::sync::Arc::new(Mutex::new((Vec::new(), Vec::new())));
        let output = data.clone();
        self.stream_query(
            sql,
            Box::new(move |event| {
                let mut data = output.lock().unwrap();
                match event {
                    QueryEvent::Columns(columns) => data.0 = columns,
                    QueryEvent::Row(row) => data.1.push(row),
                    QueryEvent::End => (),
                }
                Ok(())
            }),
        )?;
        let (columns, rows) = std::mem::take(&mut *data.lock().unwrap());
        Ok(QueryResult {
            columns,
            rows,
            truncated: false,
            elapsed: start.elapsed(),
        })
    }
    /// Native ALTER statements, applied atomically with an FK check before
    /// commit. Failure must leave the original schema and rows intact.
    fn apply_schema_changes(&self, statements: &[String]) -> DbResult<Duration>;
    /// Non-SELECT statement; returns affected-row count (-1 if unknown).
    fn execute(&self, sql: &str) -> DbResult<(i64, Duration)>;
    fn execute_params(&self, sql: &str, params: &[PValue]) -> DbResult<(i64, Duration)>;
    /// Own a transaction, refusing to join an existing caller's work.
    fn begin_transaction(&self) -> DbResult<()> {
        self.execute("BEGIN").map(|_| ())
    }
    fn commit_transaction(&self) -> DbResult<()> {
        self.execute("COMMIT").map(|_| ())
    }
    fn rollback_transaction(&self) -> DbResult<()> {
        self.execute("ROLLBACK").map(|_| ())
    }
    /// Returns the record's identity after the write, including key changes.
    fn update_row(&self, table: &str, rowid: i64, changes: &[(String, PValue)]) -> DbResult<i64>;
    /// INSERT with the provided columns (omitted ones take DB defaults);
    /// returns the new rowid.
    fn insert_row(&self, table: &str, changes: &[(String, PValue)]) -> DbResult<i64>;
    fn delete_row(&self, table: &str, rowid: i64) -> DbResult<()>;
    /// FK targets declared BY `table`: (from_col, to_table, to_col).
    /// to_col is None when the FK references the parent's pk (bare
    /// `REFERENCES parent`). Powers the TABLE EDITOR round-trip.
    fn outgoing_fks(&self, table: &str) -> Vec<(String, String, Option<String>)> {
        let q = self
            .query(&format!(
                "SELECT \"from\", \"table\", \"to\" FROM pragma_foreign_key_list({})",
                sql_str(table)
            ))
            .unwrap_or_else(|_| QueryResult {
                columns: Vec::new(),
                rows: Vec::new(),
                truncated: false,
                elapsed: Duration::ZERO,
            });
        q.rows
            .iter()
            .filter_map(|r| match (r.first(), r.get(1), r.get(2)) {
                (Some(PValue::Text(from)), Some(PValue::Text(to_table)), to_col) => Some((
                    from.clone(),
                    to_table.clone(),
                    match to_col {
                        Some(PValue::Text(c)) => Some(c.clone()),
                        _ => None,
                    },
                )),
                _ => None,
            })
            .collect()
    }

    /// Worst dbhealth_report status if the view exists and is readable
    /// ("ok" | "warn" | "attention" | "no data"), else None.
    fn health(&self) -> Option<String>;

    /// Tables whose declared foreign keys point AT `parent`:
    /// (child_table, child_col, parent_col). parent_col falls back to
    /// the parent's rowid pk when the FK names no column. Works on any
    /// backend — it's plain SQL over pragma table-functions.
    fn child_links(&self, parent: &str) -> Vec<(String, String, String)> {
        let Ok(tables) = self.tables() else {
            return Vec::new();
        };
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
                let (PValue::Text(to_table), PValue::Text(from_col)) = (&row[0], &row[1]) else {
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

/// Only ordinary rowid tables have the identity guarantees EDIT needs.
/// In particular, a real column named rowid does not establish identity
/// on a view or a WITHOUT ROWID table. Include hidden/generated names
/// when checking which aliases are shadowed.
pub(crate) fn resolve_rowid_column(db: &dyn DbLink, table: &str) -> DbResult<Option<String>> {
    let physical = db.query(&format!(
        "SELECT 1 FROM pragma_table_list WHERE schema = 'main' AND name = {} COLLATE NOCASE \
         AND type = 'table' AND wr = 0",
        sql_str(table)
    ))?;
    if physical.rows.is_empty() {
        return Ok(None);
    }
    let columns = db.query(&format!(
        "SELECT name FROM pragma_table_xinfo({})",
        sql_str(table)
    ))?;
    Ok(["rowid", "_rowid_", "oid"].into_iter().find(|alias| {
        !columns.rows.iter().any(|r| matches!(r.first(), Some(PValue::Text(name)) if name.eq_ignore_ascii_case(alias)))
    }).map(str::to_owned))
}

/// INTEGER PRIMARY KEY is automatic only when it aliases the rowid.
/// DESC primary keys and composite keys have a separate primary-key index.
pub(crate) fn automatic_key(
    db: &dyn DbLink,
    table: &str,
    columns: &[ColumnInfo],
) -> Option<String> {
    let keys: Vec<_> = columns.iter().filter(|c| c.pk).collect();
    if keys.len() != 1 || !keys[0].decl_type.eq_ignore_ascii_case("INTEGER") || !db.has_rowid(table)
    {
        return None;
    }
    let indexes = db
        .query(&format!(
            "SELECT 1 FROM pragma_index_list({}) WHERE origin='pk'",
            sql_str(table)
        ))
        .ok()?;
    indexes.rows.is_empty().then(|| keys[0].name.clone())
}

/// Guard the identity in the write statement itself: if another
/// connection shadows our cached alias, the write must affect no rows.
/// Qualifying the identifier also disables SQLite's quoted-string fallback.
pub(crate) fn rowid_predicate(table: &str, alias: &str, parameter: &str) -> String {
    format!(
        "{}.{} = {parameter} AND NOT EXISTS \
         (SELECT 1 FROM pragma_table_xinfo({}) WHERE name = {} COLLATE NOCASE) \
         AND EXISTS (SELECT 1 FROM pragma_table_list WHERE schema = 'main' \
         AND name = {} COLLATE NOCASE AND type = 'table' AND wr = 0)",
        EmbeddedDb::quote(table),
        EmbeddedDb::quote(alias),
        sql_str(table),
        sql_str(alias),
        sql_str(table)
    )
}

pub struct EmbeddedDb {
    conn: Connection,
    name: String,
    readonly: bool,
    // Mutexes, not RefCells: the whole backend moves to the worker
    // thread (worker.rs), which needs Send. They are never contended —
    // one thread owns the backend — so lock().unwrap() never blocks
    // and poisoning would require a panic inside a HashMap op.
    rowid_cache: Mutex<HashMap<String, Option<String>>>,
    anchors: Mutex<HashMap<String, AnchorIndex>>,
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
        Ok((
            Page {
                rows,
                rowids: Some(rowids),
            },
            total,
        ))
    } else {
        Ok((Page { rows, rowids: None }, total))
    }
}

/// Sparse absolute-position → rowid anchors for keyset paging.
///
/// OFFSET windows rescan from row 0 on every PgDn (linear in depth);
/// once a window's rowids are known, later windows can start FROM the
/// nearest anchor (`WHERE rowid >= ? ORDER BY rowid`) and skip a bounded
/// remainder instead. Anchors are only valid while positions are stable,
/// so backends drop them on any write to the table.
///
/// Embedded-only: the remote backend keeps OFFSET (another sqld client
/// may move positions under us — correctness first).
#[derive(Debug, Default)]
pub(crate) struct AnchorIndex {
    map: std::collections::BTreeMap<i64, i64>,
}

impl AnchorIndex {
    /// Anchor every Nth position; cap total anchors (memory bound).
    const STRIDE: i64 = 64;
    const CAP: usize = 4096;
    /// Nearest anchor at/before `offset` → (start_rowid, rows_to_skip).
    /// Skip is bounded by STRIDE when flight is sequential.
    fn plan(&self, offset: i64) -> Option<(i64, i64)> {
        self.map
            .range(..=offset)
            .next_back()
            .map(|(a_off, a_row)| (*a_row, offset - a_off))
    }

    /// Record anchors for a fetched window: `rowids[i]` sits at
    /// absolute position `offset + i` (caller guarantees rowid order).
    fn observe(&mut self, offset: i64, rowids: &[i64]) {
        if rowids.is_empty() {
            return;
        }
        for (i, r) in rowids.iter().enumerate() {
            let pos = offset + i as i64;
            if pos % Self::STRIDE == 0 {
                self.map.insert(pos, *r);
            }
        }
        while self.map.len() > Self::CAP {
            self.map.pop_first();
        }
    }
}

impl EmbeddedDb {
    /// Open a database file. If PHOSPHOR_EXT names a loadable extension
    /// (e.g. libtimeless_ext.so), load it — capability, not dependency:
    /// failures are reported but the db still opens.
    pub fn open(path: &str) -> DbResult<(Self, Option<String>)> {
        Self::open_with_mode(path, false)
    }

    pub fn open_with_mode(path: &str, readonly: bool) -> DbResult<(Self, Option<String>)> {
        let flags = if readonly {
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX
        } else {
            rusqlite::OpenFlags::default()
        };
        let conn = Connection::open_with_flags(path, flags).map_err(|e| e.to_string())?;
        // Declared foreign keys should MEAN something: enforce them.
        // (SQLite defaults to off; existing orphan rows only surface
        // as errors on writes that would violate a constraint.)
        let _ = conn.execute_batch("PRAGMA foreign_keys = ON");
        if readonly {
            conn.execute_batch("PRAGMA query_only = ON")
                .map_err(|e| e.to_string())?;
        }
        let mut warning = None;
        if let Ok(ext) = std::env::var("PHOSPHOR_EXT") {
            if !ext.is_empty() {
                let loaded = unsafe {
                    let _guard =
                        rusqlite::LoadExtensionGuard::new(&conn).map_err(|e| e.to_string())?;
                    conn.load_extension(&ext, None::<&str>)
                };
                if let Err(e) = loaded {
                    warning = Some(format!("PHOSPHOR_EXT not loaded: {e}"));
                }
            }
        }
        if readonly {
            // Restore the setting after extension initialization too.
            conn.execute_batch("PRAGMA query_only = ON")
                .map_err(|e| e.to_string())?;
            // query_only also protects temp/attached databases and a scratch
            // connection. The authorizer prevents SQL from undoing it or
            // attaching another file. SQLite itself rejects writes, including
            // CTEs and RETURNING statements submitted through query().
            conn.authorizer(Some(|ctx: rusqlite::hooks::AuthContext<'_>| {
                use rusqlite::hooks::{AuthAction, Authorization};
                match ctx.action {
                    AuthAction::Attach { .. } | AuthAction::Detach { .. } => Authorization::Deny,
                    AuthAction::Pragma {
                        pragma_name,
                        pragma_value,
                    } => {
                        let name = pragma_name.to_ascii_lowercase();
                        let metadata = matches!(
                            name.as_str(),
                            "table_info"
                                | "table_xinfo"
                                | "table_list"
                                | "index_list"
                                | "index_info"
                                | "index_xinfo"
                                | "foreign_key_list"
                                | "foreign_key_check"
                                | "integrity_check"
                                | "quick_check"
                        );
                        let setting = pragma_value.is_none()
                            && matches!(
                                name.as_str(),
                                "query_only"
                                    | "foreign_keys"
                                    | "database_list"
                                    | "compile_options"
                                    | "data_version"
                                    | "schema_version"
                                    | "user_version"
                                    | "page_count"
                                    | "page_size"
                                    | "freelist_count"
                                    | "pragma_list"
                                    | "function_list"
                                    | "module_list"
                                    | "collation_list"
                            );
                        if metadata || setting {
                            Authorization::Allow
                        } else {
                            Authorization::Deny
                        }
                    }
                    AuthAction::Function { function_name }
                        if function_name.eq_ignore_ascii_case("load_extension")
                            || function_name.eq_ignore_ascii_case("writefile") =>
                    {
                        Authorization::Deny
                    }
                    _ => Authorization::Allow,
                }
            }));
        }
        Ok((
            EmbeddedDb {
                conn,
                name: path.to_owned(),
                readonly,
                rowid_cache: Mutex::new(HashMap::new()),
                anchors: Mutex::new(HashMap::new()),
            },
            warning,
        ))
    }

    fn quote(ident: &str) -> String {
        format!("\"{}\"", ident.replace('"', "\"\""))
    }

    /// A failed cardinality check must roll back trigger side effects
    /// too. SAVEPOINT works inside a caller-owned transaction.
    fn write_one(
        &self,
        verb: &str,
        write: impl FnOnce() -> rusqlite::Result<usize>,
    ) -> DbResult<()> {
        self.conn
            .execute_batch("SAVEPOINT phosphor_row_write")
            .map_err(|e| e.to_string())?;
        let result = write().map_err(|e| e.to_string()).and_then(|n| {
            if n == 1 {
                self.conn
                    .execute_batch("RELEASE phosphor_row_write")
                    .map_err(|e| e.to_string())
            } else {
                Err(format!(
                    "expected to {verb} 1 row, affected {n}; refresh the table"
                ))
            }
        });
        self.anchors.lock().unwrap().clear();
        match result {
            Ok(()) => Ok(()),
            Err(error) => match self
                .conn
                .execute_batch("ROLLBACK TO phosphor_row_write; RELEASE phosphor_row_write")
            {
                Ok(()) => Err(error),
                Err(rollback) => Err(format!("{error}; rollback: {rollback}")),
            },
        }
    }

    fn collect_rows(
        stmt: &mut rusqlite::Statement<'_>,
        params: &[&dyn rusqlite::ToSql],
        cap: usize,
    ) -> DbResult<(Vec<Vec<PValue>>, bool)> {
        let mut rows_out = Vec::new();
        let mut truncated = false;
        let mut rows = stmt.query(params).map_err(|e| e.to_string())?;
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            if rows_out.len() >= cap {
                truncated = true;
                break;
            }
            // Live count from the statement this row came from, not a
            // pre-step snapshot: a cached statement re-prepares on a
            // schema change (ALTER TABLE) and its width shifts under us.
            let ncols = row.as_ref().column_count();
            let mut out = Vec::with_capacity(ncols);
            for i in 0..ncols {
                out.push(PValue::from_ref(row.get_ref(i).map_err(|e| e.to_string())?));
            }
            rows_out.push(out);
        }
        Ok((rows_out, truncated))
    }

    /// Nearest anchor at/before `offset`, if any.
    fn anchor_plan(&self, table: &str, offset: i64) -> Option<(i64, i64)> {
        self.anchors
            .lock()
            .unwrap()
            .get(table)
            .and_then(|a| a.plan(offset))
    }

    /// Record a fetched window's rowids (absolute positions).
    fn anchor_observe(&self, table: &str, offset: i64, rowids: &[i64]) {
        self.anchors
            .lock()
            .unwrap()
            .entry(table.to_owned())
            .or_default()
            .observe(offset, rowids);
    }

    /// Positions moved (write path drops the WHOLE index: cascades and
    /// triggers can shift other tables' positions too).
    #[allow(dead_code)]
    fn anchor_clear(&self, table: &str) {
        self.anchors.lock().unwrap().remove(table);
    }

    /// Fetch `limit` rows from `offset` via a rowid anchor, skipping
    /// the `skip` rows between the anchor and the window.
    fn keyset_page(
        &self,
        table: &str,
        q: &str,
        start_rowid: i64,
        skip: i64,
        offset: i64,
        limit: i64,
    ) -> DbResult<Page> {
        let fetch = skip + limit;
        let mut stmt = self
            .conn
            .prepare_cached(&format!(
                "SELECT {q}.{id}, * FROM {q} WHERE {q}.{id} >= ?1 ORDER BY {q}.{id} LIMIT ?2",
                id = Self::quote(
                    &self
                        .rowid_column(table)?
                        .ok_or("no unambiguous row identity")?
                )
            ))
            .map_err(|e| e.to_string())?;
        let (all, _) =
            Self::collect_rows(&mut stmt, &[&start_rowid, &fetch], fetch.max(0) as usize)?;
        if (all.len() as i64) < skip {
            return Err("anchor overshot: table shrank under us".into());
        }
        let mut rows: Vec<Vec<PValue>> = all
            .into_iter()
            .skip(skip as usize)
            .take(limit.max(0) as usize)
            .collect();
        let mut rowids = Vec::with_capacity(rows.len());
        for row in &mut rows {
            match row.remove(0) {
                PValue::Int(id) => rowids.push(id),
                _ => return Err("rowid was not an integer".into()),
            }
        }
        self.anchor_observe(table, offset, &rowids);
        Ok(Page {
            rows,
            rowids: Some(rowids),
        })
    }

    /// Plain OFFSET fetch in pinned rowid order; observes anchors.
    fn offset_page(&self, table: &str, q: &str, offset: i64, limit: i64) -> DbResult<Page> {
        let mut stmt = self
            .conn
            .prepare_cached(&format!(
                "SELECT {q}.{id}, * FROM {q} ORDER BY {q}.{id} LIMIT ?1 OFFSET ?2",
                id = Self::quote(
                    &self
                        .rowid_column(table)?
                        .ok_or("no unambiguous row identity")?
                )
            ))
            .map_err(|e| e.to_string())?;
        let (mut rows, _) =
            Self::collect_rows(&mut stmt, &[&limit, &offset], limit.max(0) as usize)?;
        let mut rowids = Vec::with_capacity(rows.len());
        for row in &mut rows {
            match row.remove(0) {
                PValue::Int(id) => rowids.push(id),
                _ => return Err("rowid was not an integer".into()),
            }
        }
        self.anchor_observe(table, offset, &rowids);
        Ok(Page {
            rows,
            rowids: Some(rowids),
        })
    }
}

impl DbLink for EmbeddedDb {
    fn save_scratch(&mut self, path: &str) -> DbResult<()> {
        self.require_writable()?;
        if self.name != ":memory:" {
            return Err("this database already saves to its file".into());
        }
        if !self.conn.is_autocommit() {
            return Err(
                "finish the current transaction with COMMIT or ROLLBACK before saving the database"
                    .into(),
            );
        }
        if path.is_empty() || path == ":memory:" || path.starts_with("file:") {
            return Err("enter a new database filename, such as crm.db".into());
        }
        let attached: i64 = self
            .conn
            .query_row(
                "SELECT count(*) FROM pragma_database_list WHERE name NOT IN ('main','temp')",
                [],
                |row| row.get(0),
            )
            .map_err(|e| e.to_string())?;
        if attached > 0 {
            return Err(
                "Save Database copies the main database; DETACH attached databases before saving"
                    .into(),
            );
        }
        let temporary: i64 = self
            .conn
            .query_row("SELECT count(*) FROM temp.sqlite_schema", [], |row| {
                row.get(0)
            })
            .map_err(|e| e.to_string())?;
        if temporary > 0 {
            return Err("Save Database copies the main database; move or drop temporary objects before saving".into());
        }
        let path = std::path::Path::new(path);
        if std::fs::symlink_metadata(path).is_ok() {
            return Err("that file already exists; choose a new filename".into());
        }
        let (output, file) = crate::output::AtomicOutput::create(path)?;
        self.conn
            .backup("main", output.temporary_path(), None)
            .map_err(|e| e.to_string())?;
        file.sync_all().map_err(|e| e.to_string())?;
        drop(file);
        output.publish_new()?;
        // Open by the published filename so subsequent journals and writes
        // belong to that file. Keep scratch alive if opening fails.
        let (saved, _) =
            Self::open(path.to_str().ok_or("filename must be UTF-8")?).map_err(|e| {
                format!(
                    "database file was saved but could not be opened: {e}; scratch is still open"
                )
            })?;
        *self = saved;
        Ok(())
    }

    fn readonly(&self) -> bool {
        self.readonly
    }
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
            .prepare_cached(COLUMN_INFO_SQL)
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([table], |r| {
                Ok(ColumnInfo {
                    name: r.get(1)?,
                    decl_type: r.get::<_, Option<String>>(2)?.unwrap_or_default(),
                    notnull: r.get::<_, i64>(3)? != 0,
                    pk: r.get::<_, i64>(5)? != 0,
                    generated: r.get::<_, i64>(6)? >= 2,
                    dflt_value: r.get(4)?,
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

    fn rowid_column(&self, table: &str) -> DbResult<Option<String>> {
        if let Some(known) = self.rowid_cache.lock().unwrap().get(table) {
            return Ok(known.clone());
        }
        // Errors are transient; only cache a successfully inspected schema.
        let alias = resolve_rowid_column(self, table)?;
        self.rowid_cache
            .lock()
            .unwrap()
            .insert(table.to_owned(), alias.clone());
        Ok(alias)
    }

    fn page(&self, table: &str, offset: i64, limit: i64) -> DbResult<Page> {
        let q = Self::quote(table);
        if !self.has_rowid(table) {
            // Views and WITHOUT ROWID tables: plain OFFSET (unchanged).
            let mut stmt = self
                .conn
                .prepare_cached(&format!("SELECT * FROM {q} LIMIT ?1 OFFSET ?2"))
                .map_err(|e| e.to_string())?;
            let (rows, _) =
                Self::collect_rows(&mut stmt, &[&limit, &offset], limit.max(0) as usize)?;
            return Ok(Page { rows, rowids: None });
        }
        // Keyset fast path: resume from the nearest anchor at/before
        // the wanted offset (sequential flight reuses the previous
        // window's tail anchor with a bounded skip).
        if let Some((start_rowid, skip)) = self.anchor_plan(table, offset) {
            if let Ok(page) = self.keyset_page(table, &q, start_rowid, skip, offset, limit) {
                return Ok(page);
            }
        }
        // OFFSET fallback (also seeds anchors for the flight ahead).
        // ORDER BY rowid pins grid order so anchors stay coherent.
        self.offset_page(table, &q, offset, limit)
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
        let alias = self.rowid_column(table)?;
        let with_rowid = alias.is_some();
        let select = if let Some(alias) = alias {
            let id = Self::quote(&alias);
            format!("SELECT {q}.{id}, *, count(*) OVER () AS _total FROM {q} ORDER BY {q}.{id}")
        } else {
            format!("SELECT *, count(*) OVER () AS _total FROM {q}")
        };
        let mut stmt = match self
            .conn
            .prepare_cached(&format!("{select} LIMIT {limit} OFFSET {offset}"))
        {
            Ok(s) => s,
            Err(_) => return fallback(),
        };
        let (rows, _) = match Self::collect_rows(&mut stmt, &[], limit.max(0) as usize) {
            Ok(r) => r,
            Err(_) => return fallback(),
        };
        if rows.is_empty() {
            // A zero/negative limit on a non-empty table says nothing
            // about the total: only trust the shortcut for a real fetch.
            if offset == 0 && limit > 0 {
                // Open on an empty table: total is exactly 0, no count needed.
                let page = Page {
                    rows: Vec::new(),
                    rowids: with_rowid.then(Vec::new),
                };
                return Ok((page, 0));
            }
            return fallback();
        }
        match strip_window_total(rows, with_rowid) {
            Ok(ok) => Ok(ok),
            Err(_) => fallback(),
        }
    }

    fn stream_query(&self, sql: &str, mut sink: RowSink) -> DbResult<usize> {
        let sql = crate::sql::select_source(sql)?;
        let mut stmt = self.conn.prepare(&sql).map_err(|e| e.to_string())?;
        if !stmt.readonly() {
            return Err("output source must be a read-only SELECT".into());
        }
        sink(QueryEvent::Columns(
            stmt.column_names()
                .iter()
                .map(|s| (*s).to_owned())
                .collect(),
        ))?;
        let mut rows = stmt.query([]).map_err(|e| e.to_string())?;
        let mut count = 0;
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            let values = (0..row.as_ref().column_count())
                .map(|i| {
                    row.get_ref(i)
                        .map(PValue::from_ref)
                        .map_err(|e| e.to_string())
                })
                .collect::<DbResult<Vec<_>>>()?;
            sink(QueryEvent::Row(values))?;
            count += 1;
        }
        sink(QueryEvent::End)?;
        Ok(count)
    }

    fn query(&self, sql: &str) -> DbResult<QueryResult> {
        let start = Instant::now();
        // Execute the statement unchanged; PRAGMA, VALUES, EXPLAIN, comments,
        // and the user's own LIMIT all retain SQLite's normal semantics.
        let sql = crate::sql::single_query(sql)?;
        let mut stmt = self.conn.prepare(sql).map_err(|e| {
            if self.readonly {
                format!("read-only query: {e}")
            } else {
                e.to_string()
            }
        })?;
        let columns: Vec<String> = stmt.column_names().into_iter().map(str::to_owned).collect();
        let (mut rows, _) = Self::collect_rows(&mut stmt, &[], QUERY_CAP + 1).map_err(|e| {
            if self.readonly {
                format!("read-only query: {e}")
            } else {
                e
            }
        })?;
        let truncated = rows.len() > QUERY_CAP;
        rows.truncate(QUERY_CAP);
        Ok(QueryResult {
            columns,
            rows,
            truncated,
            elapsed: start.elapsed(),
        })
    }

    fn apply_schema_changes(&self, statements: &[String]) -> DbResult<Duration> {
        self.require_writable()?;
        if self
            .conn
            .pragma_query_value(None, "legacy_alter_table", |r| r.get::<_, bool>(0))
            .map_err(|e| e.to_string())?
        {
            return Err(
                "table editing requires PRAGMA legacy_alter_table=OFF to preserve dependencies"
                    .into(),
            );
        }
        let start = Instant::now();
        // Refuse to take over a caller's open transaction. The transaction
        // object rolls back both statement and validation failures on drop.
        let tx = self
            .conn
            .unchecked_transaction()
            .map_err(|e| e.to_string())?;
        let result = (|| {
            for sql in statements {
                tx.execute_batch(sql).map_err(|e| e.to_string())?;
            }
            let mut check = tx
                .prepare("PRAGMA foreign_key_check")
                .map_err(|e| e.to_string())?;
            if check
                .query([])
                .map_err(|e| e.to_string())?
                .next()
                .map_err(|e| e.to_string())?
                .is_some()
            {
                return Err("foreign key check failed; schema changes rolled back".into());
            }
            drop(check);
            tx.commit().map_err(|e| e.to_string())?;
            Ok(start.elapsed())
        })();
        self.rowid_cache.lock().unwrap().clear();
        self.anchors.lock().unwrap().clear();
        result
    }

    fn execute(&self, sql: &str) -> DbResult<(i64, Duration)> {
        self.require_writable()?;
        let start = Instant::now();
        // Multi-statement input goes through execute_batch (count
        // unknown): single-statement execute would prepare later
        // statements eagerly and fail name resolution against objects
        // the earlier statements create. A ';' inside a string literal
        // false-positives into batch — harmless, just loses the count.
        let body = sql.trim().trim_end_matches(';');
        // DDL may change rowid-ness; drop cached probes (cheap: only on write/DDL path).
        // Any execute may move positions: drop all paging anchors too
        // (they rebuild within a few windows; prompt SELECTs never
        // reach here — they go through query()).
        let lower = sql.to_ascii_lowercase();
        if lower.contains("create")
            || lower.contains("drop")
            || lower.contains("alter")
            || lower.contains("vacuum")
        {
            self.rowid_cache.lock().unwrap().clear();
        }
        self.anchors.lock().unwrap().clear();
        if body.contains(';') {
            self.conn.execute_batch(sql).map_err(|e| e.to_string())?;
            return Ok((-1, start.elapsed()));
        }
        // sqlite3_changes retains the last DML count across BEGIN,
        // COMMIT and DDL. Report zero when this statement did no writes.
        let before = self.conn.total_changes();
        match self.conn.execute(sql, []) {
            Ok(n) => Ok((
                if self.conn.total_changes() == before {
                    0
                } else {
                    n as i64
                },
                start.elapsed(),
            )),
            Err(e) => Err(e.to_string()),
        }
    }

    fn execute_params(&self, sql: &str, params: &[PValue]) -> DbResult<(i64, Duration)> {
        self.require_writable()?;
        self.rowid_cache.lock().unwrap().clear();
        self.anchors.lock().unwrap().clear();
        let start = Instant::now();
        let before = self.conn.total_changes();
        let count = self
            .conn
            .execute(sql, rusqlite::params_from_iter(params))
            .map_err(|e| e.to_string())?;
        Ok((
            if self.conn.total_changes() == before {
                0
            } else {
                count as i64
            },
            start.elapsed(),
        ))
    }

    fn update_row(&self, table: &str, rowid: i64, changes: &[(String, PValue)]) -> DbResult<i64> {
        self.require_writable()?;
        if changes.is_empty() {
            return Ok(rowid);
        }
        let alias = self
            .rowid_column(table)?
            .ok_or("no unambiguous row identity; editing is read-only")?;
        let sets: Vec<String> = changes
            .iter()
            .enumerate()
            .map(|(i, (col, _))| format!("{} = ?{}", Self::quote(col), i + 1))
            .collect();
        let sql = format!(
            "UPDATE {} SET {} WHERE {} RETURNING {}",
            Self::quote(table),
            sets.join(", "),
            rowid_predicate(table, &alias, &format!("?{}", changes.len() + 1)),
            Self::quote(&alias)
        );
        let mut stmt = self.conn.prepare_cached(&sql).map_err(|e| e.to_string())?;
        for (i, (_, v)) in changes.iter().enumerate() {
            stmt.raw_bind_parameter(i + 1, v)
                .map_err(|e| e.to_string())?;
        }
        stmt.raw_bind_parameter(changes.len() + 1, rowid)
            .map_err(|e| e.to_string())?;
        let mut identity = None;
        self.write_one("update", || {
            let mut rows = stmt.raw_query();
            let mut count = 0;
            while let Some(row) = rows.next()? {
                identity = Some(row.get(0)?);
                count += 1;
            }
            Ok(count)
        })?;
        identity.ok_or_else(|| "updated record has no usable identity".into())
    }

    fn insert_row(&self, table: &str, changes: &[(String, PValue)]) -> DbResult<i64> {
        self.require_writable()?;
        let sql = if changes.is_empty() {
            format!("INSERT INTO {} DEFAULT VALUES", Self::quote(table))
        } else {
            let cols: Vec<String> = changes.iter().map(|(c, _)| Self::quote(c)).collect();
            let marks: Vec<String> = (1..=changes.len()).map(|i| format!("?{i}")).collect();
            format!(
                "INSERT INTO {} ({}) VALUES ({})",
                Self::quote(table),
                cols.join(", "),
                marks.join(", ")
            )
        };
        let mut stmt = self.conn.prepare_cached(&sql).map_err(|e| e.to_string())?;
        for (i, (_, v)) in changes.iter().enumerate() {
            stmt.raw_bind_parameter(i + 1, v)
                .map_err(|e| e.to_string())?;
        }
        stmt.raw_execute().map_err(|e| e.to_string())?;
        // Cascade/trigger writes may touch other tables: drop all anchors.
        self.anchors.lock().unwrap().clear();
        Ok(self.conn.last_insert_rowid())
    }

    fn delete_row(&self, table: &str, rowid: i64) -> DbResult<()> {
        self.require_writable()?;
        let alias = self
            .rowid_column(table)?
            .ok_or("no unambiguous row identity; deleting is read-only")?;
        self.write_one("delete", || {
            self.conn.execute(
                &format!(
                    "DELETE FROM {} WHERE {}",
                    Self::quote(table),
                    rowid_predicate(table, &alias, "?1")
                ),
                [rowid],
            )
        })
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
        let Ok(tables) = self.tables() else {
            return Vec::new();
        };
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
        let Ok(q) = self.query(&parts.join(" UNION ALL ")) else {
            return Vec::new();
        };
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

    #[test]
    fn complete_outputs_include_rows_beyond_the_preview_cap() {
        let (db, _) = EmbeddedDb::open(":memory:").unwrap();
        crate::test_support::assert_complete_output(&db);
    }

    #[test]
    fn embedded_transactions_and_saved_scripts_round_trip() {
        let (db, _) = EmbeddedDb::open(":memory:").unwrap();
        crate::test_support::assert_transaction_and_script_workflows(&db);
    }

    #[test]
    fn native_schema_changes_preserve_constraints_and_roll_back_failures() {
        let (db, _) = EmbeddedDb::open(":memory:").unwrap();
        db.execute(crate::test_support::SCHEMA_SQL).unwrap();
        crate::test_support::assert_native_schema_safety(&db);
        db.execute("BEGIN; INSERT INTO audit VALUES ('pending')")
            .unwrap();
        assert!(db
            .apply_schema_changes(&["ALTER TABLE audit ADD COLUMN extra TEXT".into()])
            .is_err());
        assert!(!db.conn.is_autocommit(), "caller transaction retained");
        assert_eq!(db.count("audit").unwrap(), 3);
        db.execute("ROLLBACK").unwrap();
        assert_eq!(db.count("audit").unwrap(), 2);
        db.execute("PRAGMA legacy_alter_table=ON").unwrap();
        assert!(db
            .apply_schema_changes(&["ALTER TABLE goods RENAME TO broken".into()])
            .is_err());
        assert_eq!(db.count("goods").unwrap(), 2);
    }

    #[test]
    fn schema_changes_check_foreign_keys_before_commit() {
        let (db, _) = EmbeddedDb::open(":memory:").unwrap();
        db.execute(
            "PRAGMA foreign_keys=OFF; CREATE TABLE p(id INTEGER PRIMARY KEY);
            CREATE TABLE c(pid INTEGER REFERENCES p(id)); INSERT INTO c VALUES (7);",
        )
        .unwrap();
        let error = db
            .apply_schema_changes(&["ALTER TABLE c ADD COLUMN note TEXT".into()])
            .unwrap_err();
        assert!(error.contains("foreign key check"), "{error}");
        assert_eq!(db.columns("c").unwrap().len(), 1);
        assert_eq!(db.count("c").unwrap(), 1);
        assert!(db.conn.is_autocommit());
        assert_eq!(
            db.conn
                .query_row("PRAGMA foreign_keys", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            0,
            "connection settings retained"
        );
    }

    #[test]
    fn readonly_connection_blocks_every_write_path_and_pragma_escape() {
        let file = crate::test_support::TestDb::new();
        file.connect().execute_batch("CREATE TABLE t(id INTEGER PRIMARY KEY, name TEXT); INSERT INTO t VALUES (1, 'Ada')").unwrap();
        let before = std::fs::read(file.path()).unwrap();
        let (db, _) = EmbeddedDb::open_with_mode(file.path(), true).unwrap();
        assert!(db.readonly());
        assert_eq!(db.columns("t").unwrap().len(), 2);
        assert_eq!(db.open_window("t", 0, 10).unwrap().1, 1);
        assert!(db.query("WITH x AS (SELECT 1) SELECT * FROM t").is_ok());
        for sql in [
            "WITH x AS (SELECT 1) INSERT INTO t(name) SELECT 'bad' FROM x RETURNING * -- limit",
            "UPDATE t SET name='bad' RETURNING * -- limit",
            "DELETE FROM t RETURNING * -- limit",
            "PRAGMA query_only=OFF -- limit",
            "PRAGMA user_version=99 -- limit",
            "ATTACH ':memory:' AS extra -- limit",
        ] {
            assert!(db.query(sql).is_err(), "allowed {sql}");
        }
        assert!(db
            .execute("PRAGMA query_only=OFF; INSERT INTO t VALUES (2, 'bad')")
            .is_err());
        assert!(db
            .update_row("t", 1, &[("name".into(), PValue::Text("bad".into()))])
            .is_err());
        assert!(db.insert_row("t", &[]).is_err());
        assert!(db.delete_row("t", 1).is_err());
        assert!(db
            .apply_schema_changes(&["ALTER TABLE t ADD COLUMN extra TEXT".into()])
            .is_err());
        // Even an internal caller bypassing DbLink cannot disable the guard.
        assert!(db.conn.execute_batch("PRAGMA query_only=OFF").is_err());
        assert!(db
            .conn
            .execute_batch("CREATE TEMP TABLE scratch(x)")
            .is_err());
        crate::store::pref_set(&db, "theme", "amber");
        assert_eq!(db.tables().unwrap().len(), 1);
        assert_eq!(std::fs::read(file.path()).unwrap(), before);
    }

    #[test]
    fn readonly_open_does_not_create_missing_file_and_protects_scratch() {
        let missing = crate::test_support::TestDb::new();
        assert!(EmbeddedDb::open_with_mode(missing.path(), true).is_err());
        assert!(!std::path::Path::new(missing.path()).exists());
        let (db, _) = EmbeddedDb::open_with_mode(":memory:", true).unwrap();
        assert!(db.query("SELECT 1").is_ok());
        assert!(db.conn.execute_batch("CREATE TABLE t(x)").is_err());
    }

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
    fn shadowed_rowid_aliases_never_target_another_record() {
        for declarations in ["rowid INTEGER", "RoWiD INTEGER, _RoWiD_ INTEGER"] {
            let (db, _) = EmbeddedDb::open(":memory:").unwrap();
            db.execute(&format!(
                "CREATE TABLE t({declarations}, name TEXT, PRIMARY KEY(name));
                INSERT INTO t(name, rowid) VALUES ('Alice',7), ('Bob',7)"
            ))
            .unwrap();
            let (page, total) = db.open_window("t", 0, 10).unwrap();
            assert_eq!(total, 2);
            assert_eq!(page.rowids, Some(vec![1, 2]));
            assert_eq!(db.page("t", 0, 1).unwrap().rowids, Some(vec![1]));
            assert_eq!(db.page("t", 1, 1).unwrap().rowids, Some(vec![2]));
            db.update_row("t", 1, &[("name".into(), PValue::Text("Alicia".into()))])
                .unwrap();
            db.delete_row("t", 2).unwrap();
            assert_eq!(
                db.query("SELECT name, rowid FROM t").unwrap().rows,
                vec![vec![PValue::Text("Alicia".into()), PValue::Int(7)]]
            );
        }
    }

    #[test]
    fn cached_rowid_cannot_become_a_multirow_write_after_schema_change() {
        let (db, _) = EmbeddedDb::open(":memory:").unwrap();
        db.execute("CREATE TABLE t(name TEXT); INSERT INTO t VALUES('Alice'),('Bob')")
            .unwrap();
        assert_eq!(db.page("t", 0, 10).unwrap().rowids, Some(vec![1, 2]));
        // Bypass application cache invalidation, as external DDL does.
        db.conn
            .execute_batch("ALTER TABLE t ADD COLUMN rowid INTEGER DEFAULT 1")
            .unwrap();
        assert!(db
            .update_row("t", 1, &[("name".into(), PValue::Text("wrong".into()))])
            .is_err());
        assert!(db.delete_row("t", 1).is_err());
        assert_eq!(
            db.query("SELECT name FROM t ORDER BY name").unwrap().rows,
            vec![
                vec![PValue::Text("Alice".into())],
                vec![PValue::Text("Bob".into())]
            ]
        );
    }

    #[test]
    fn failed_row_write_rolls_back_trigger_effects_inside_existing_transaction() {
        let (db, _) = EmbeddedDb::open(":memory:").unwrap();
        db.execute("CREATE TABLE t(name TEXT); INSERT INTO t VALUES('Alice'); CREATE TABLE audit(message TEXT);
            CREATE TRIGGER ignore_update BEFORE UPDATE ON t BEGIN INSERT INTO audit VALUES('side effect'); SELECT RAISE(IGNORE); END;
            CREATE TRIGGER ignore_delete BEFORE DELETE ON t BEGIN INSERT INTO audit VALUES('side effect'); SELECT RAISE(IGNORE); END;
            BEGIN; INSERT INTO audit VALUES('caller')").unwrap();
        assert!(db
            .update_row("t", 1, &[("name".into(), PValue::Text("wrong".into()))])
            .is_err());
        assert!(db.delete_row("t", 1).is_err());
        assert_eq!(db.count("audit").unwrap(), 1);
        assert_eq!(db.count("t").unwrap(), 1);
        db.execute("ROLLBACK").unwrap();
        assert_eq!(db.count("audit").unwrap(), 0);
    }

    #[test]
    fn ambiguous_or_missing_row_identity_is_readonly() {
        let (db, _) = EmbeddedDb::open(":memory:").unwrap();
        db.execute(
            "CREATE TABLE shadowed(rowid INTEGER, _rowid_ INTEGER, oid INTEGER, name TEXT);
            INSERT INTO shadowed VALUES(7,7,7,'Alice'),(7,7,7,'Bob');
            CREATE TABLE keyed(rowid INTEGER, name TEXT, PRIMARY KEY(rowid,name)) WITHOUT ROWID;
            INSERT INTO keyed VALUES(7,'Alice'),(7,'Bob');
            CREATE VIEW v AS SELECT * FROM shadowed",
        )
        .unwrap();
        for table in ["shadowed", "keyed", "v"] {
            assert!(!db.has_rowid(table), "{table} has no safe row identity");
            assert!(db.page(table, 0, 10).unwrap().rowids.is_none());
            assert!(db
                .update_row(table, 7, &[("name".into(), PValue::Text("wrong".into()))])
                .is_err());
            assert!(db.delete_row(table, 7).is_err());
            assert_eq!(db.count(table).unwrap(), 2);
        }
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

    /// End-to-end proof at depth: a cold deep page rescans (OFFSET),
    /// then sequential flight makes the same region keyset-fast.
    /// Asserts warm < cold (100x+ apart — structural, not a flaky bound).
    #[test]
    fn deep_flight_is_keyset_fast() {
        let (db, _) = EmbeddedDb::open(":memory:").unwrap();
        db.execute("CREATE TABLE big(v TEXT)").unwrap();
        db.execute("BEGIN").unwrap();
        for chunk in 0..200 {
            let mut ins = String::from("INSERT INTO big(v) VALUES ");
            ins.push_str(
                &(0..1000)
                    .map(|i| format!("('r{}')", chunk * 1000 + i))
                    .collect::<Vec<_>>()
                    .join(","),
            );
            db.execute(&ins).unwrap();
        }
        db.execute("COMMIT").unwrap();
        // Cold: no anchors → OFFSET rescan.
        let t0 = std::time::Instant::now();
        let p1 = db.page("big", 190_000, 150).unwrap();
        let cold = t0.elapsed();
        assert_eq!(p1.rows.len(), 150);
        // Sequential flight to build anchors, then a deep keyset page.
        let mut off = 0;
        while off < 190_000 {
            db.page("big", off, 150).unwrap();
            off += 150;
        }
        let t0 = std::time::Instant::now();
        let p2 = db.page("big", 190_080, 150).unwrap();
        let warm = t0.elapsed();
        assert_eq!(p2.rows.len(), 150);
        assert_eq!(p2.rows[0], vec![PValue::Text("r190080".into())]);
        assert_eq!(p2.rowids.unwrap()[0], 190081);
        // No timing assert here: microsecond-scale comparisons flake
        // under scheduler jitter and the cold leg also includes
        // first-time statement preparation. The equivalence test above
        // pins correctness; the printout documents the speedup.
        eprintln!("deep page: cold OFFSET {cold:?} vs warm keyset {warm:?}");
    }

    /// #23: the DESIGN.md speed budget, enforced in release builds only
    /// (`cargo test --release perf_budget`). Debug timing is meaningless,
    /// so the test compiles out under `cfg(debug_assertions)`; the bound
    /// is deliberately loose (2 ms vs the 1 ms target) to survive a
    /// shared CI runner while still catching an order-of-magnitude
    /// regression (e.g. a lost prepare-cache or OFFSET-only paging).
    #[test]
    #[cfg(not(debug_assertions))]
    fn perf_budget_embedded_pages() {
        use std::time::Duration;
        let (db, _) = EmbeddedDb::open(":memory:").unwrap();
        db.execute("CREATE TABLE big(v TEXT)").unwrap();
        db.execute("BEGIN").unwrap();
        for chunk in 0..20 {
            let mut ins = String::from("INSERT INTO big(v) VALUES ");
            ins.push_str(
                &(0..1000)
                    .map(|i| format!("('r{}')", chunk * 1000 + i))
                    .collect::<Vec<_>>()
                    .join(","),
            );
            db.execute(&ins).unwrap();
        }
        db.execute("COMMIT").unwrap();
        // Warm the statement cache once.
        db.page("big", 0, 150).unwrap();
        let n = 120i64;
        let t0 = std::time::Instant::now();
        for i in 0..n {
            db.page("big", i * 150, 150).unwrap();
        }
        let avg = t0.elapsed() / n as u32;
        eprintln!("perf budget: avg embedded page {avg:?}");
        assert!(
            avg < Duration::from_millis(2),
            "embedded page budget blown: {avg:?} (target < 1 ms)"
        );
    }

    /// #23: worker-thread round-trip overhead stays small in release.
    #[test]
    #[cfg(not(debug_assertions))]
    fn perf_budget_worker_roundtrip() {
        use std::time::Duration;
        let (db, _) = EmbeddedDb::open(":memory:").unwrap();
        db.execute("CREATE TABLE t(x)").unwrap();
        let handle = crate::worker::spawn(Box::new(db));
        let n = 1_000i64;
        let t0 = std::time::Instant::now();
        for _ in 0..n {
            handle.query("SELECT 1").unwrap();
        }
        let avg = t0.elapsed() / n as u32;
        eprintln!("perf budget: avg worker round-trip {avg:?}");
        assert!(
            avg < Duration::from_millis(2),
            "worker round-trip budget blown: {avg:?}"
        );
    }

    /// CASCADE deletes shift OTHER tables' positions: any write drops
    /// the whole anchor index, or child pages silently show wrong rows.
    #[test]
    fn anchors_drop_on_cascade() {
        let (db, _) = EmbeddedDb::open(":memory:").unwrap();
        db.execute(
            "CREATE TABLE customers(id INTEGER PRIMARY KEY);
             CREATE TABLE orders(id INTEGER PRIMARY KEY,
                 customer_id INTEGER REFERENCES customers(id) ON DELETE CASCADE);
             INSERT INTO customers VALUES (1), (2);
             INSERT INTO orders(customer_id) VALUES (1), (1), (2);",
        )
        .unwrap();
        // Seed orders' anchors via a flight.
        db.page("orders", 0, 10).unwrap();
        // Cascading delete: orders rows for customer 1 vanish.
        db.execute("DELETE FROM customers WHERE id = 1").unwrap();
        assert!(
            !db.anchors.lock().unwrap().contains_key("orders"),
            "cascade must void unrelated tables' anchors"
        );
        // Post-cascade truth: only customer 2's order remains.
        let page = db.page("orders", 0, 10).unwrap();
        assert_eq!(page.rowids, Some(vec![3]));
        assert_eq!(page.rows.len(), 1);
    }

    /// Keyset pages must return exactly what OFFSET truth says —
    /// at every depth, including stride boundaries and past-the-end.
    #[test]
    fn keyset_pages_match_offset_truth() {
        let (db, _) = EmbeddedDb::open(":memory:").unwrap();
        db.execute("CREATE TABLE k(v TEXT)").unwrap();
        let mut ins = String::from("INSERT INTO k(v) VALUES ");
        ins.push_str(
            &(0..300)
                .map(|i| format!("('r{i}')"))
                .collect::<Vec<_>>()
                .join(","),
        );
        db.execute(&ins).unwrap();

        // Raw OFFSET truth via the (anchor-free) query path.
        let truth = |offset: i64, limit: i64| -> (Vec<i64>, Vec<String>) {
            let q = db
                .query(&format!(
                    "SELECT rowid, v FROM k ORDER BY rowid LIMIT {limit} OFFSET {offset}"
                ))
                .unwrap();
            (
                q.rows
                    .iter()
                    .map(|r| match r[0] {
                        PValue::Int(id) => id,
                        _ => panic!(),
                    })
                    .collect(),
                q.rows
                    .iter()
                    .map(|r| match &r[1] {
                        PValue::Text(t) => t.clone(),
                        _ => panic!(),
                    })
                    .collect(),
            )
        };
        for offset in [0, 1, 63, 64, 65, 100, 150, 200, 299, 300, 999] {
            let page = db.page("k", offset, 50).unwrap();
            let (ids, vals) = truth(offset, 50);
            assert_eq!(page.rowids, Some(ids), "rowids at {offset}");
            let got: Vec<String> = page
                .rows
                .iter()
                .map(|r| match &r[0] {
                    PValue::Text(t) => t.clone(),
                    _ => panic!(),
                })
                .collect();
            assert_eq!(got, vals, "rows at {offset}");
        }
    }

    /// Sequential flight populates anchors; an exact-stride offset
    /// then plans with zero skip (pure keyset, no rescan).
    #[test]
    fn anchors_populate_and_plan_exact() {
        let (db, _) = EmbeddedDb::open(":memory:").unwrap();
        db.execute("CREATE TABLE k(v TEXT)").unwrap();
        let mut ins = String::from("INSERT INTO k(v) VALUES ");
        ins.push_str(
            &(0..300)
                .map(|i| format!("('r{i}')"))
                .collect::<Vec<_>>()
                .join(","),
        );
        db.execute(&ins).unwrap();
        // Fly forward in windows like PgDn-hold does.
        let mut off = 0;
        while off < 300 {
            db.page("k", off, 50).unwrap();
            off += 50;
        }
        let anchors = db.anchors.lock().unwrap();
        let idx = anchors.get("k").expect("flight records anchors");
        assert!(!idx.map.is_empty());
        // 128 is stride-aligned: exact anchor, skip 0, rowid 129.
        assert_eq!(idx.plan(128), Some((129, 0)));
        // Unaligned: bounded skip to the previous stride anchor.
        assert_eq!(idx.plan(100), Some((65, 36)));
    }

    /// Writes void anchors (positions move); views never anchor.
    #[test]
    fn anchors_invalidate_on_write() {
        let (db, _) = EmbeddedDb::open(":memory:").unwrap();
        db.execute("CREATE TABLE k(v TEXT)").unwrap();
        db.execute("INSERT INTO k(v) VALUES ('a'), ('b')").unwrap();
        db.execute("CREATE VIEW v AS SELECT v AS letter FROM k")
            .unwrap();
        db.page("k", 0, 10).unwrap();
        assert!(db.anchors.lock().unwrap().contains_key("k"));
        // A view page records nothing (no rowids to anchor on).
        db.page("v", 0, 10).unwrap();
        assert!(!db.anchors.lock().unwrap().contains_key("v"));
        db.delete_row("k", 1).unwrap();
        assert!(
            !db.anchors.lock().unwrap().contains_key("k"),
            "delete voids"
        );
        // Content after the gap: positions shift, rowids show the hole.
        let page = db.page("k", 0, 10).unwrap();
        assert_eq!(page.rowids, Some(vec![2]));
        assert_eq!(page.rows.len(), 1);
        db.page("k", 0, 10).unwrap();
        db.execute("INSERT INTO k(v) VALUES ('z')").unwrap();
        assert!(
            !db.anchors.lock().unwrap().contains_key("k"),
            "execute voids"
        );
    }

    /// Anchor memory stays bounded no matter how far the flight goes.
    #[test]
    fn anchor_index_memory_bounded() {
        let mut idx = AnchorIndex::default();
        // 300k positions → 4688 stride anchors → evicted down to CAP.
        let rowids: Vec<i64> = (1..=300_000).collect();
        idx.observe(0, &rowids);
        assert_eq!(idx.map.len(), AnchorIndex::CAP);
        // Oldest survivors still plan exactly; evicted head is gone.
        assert_eq!(idx.plan(37_888), Some((37_889, 0)));
        assert_eq!(idx.plan(0), None);
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
        db.execute("CREATE VIEW v AS SELECT v AS letter FROM t")
            .unwrap();
        let (page, total) = db.open_window("v", 0, 10).unwrap();
        assert_eq!((total, page.rows.len()), (3, 3));
        assert_eq!(page.rowids, None);
        // Empty table at offset 0: total exactly 0, no count query.
        db.execute("CREATE TABLE e(id INTEGER PRIMARY KEY)")
            .unwrap();
        let (page, total) = db.open_window("e", 0, 10).unwrap();
        assert_eq!((total, page.rows.len()), (0, 0));
        // Past-the-end window falls back to count (total still right).
        let (_, total) = db.open_window("t", 99, 10).unwrap();
        assert_eq!(total, 3);
    }

    /// A cached window statement must re-read its width after a schema
    /// change: `column_count()` snapshotted before the first step went
    /// stale once `ALTER TABLE ... DROP COLUMN` shrank the result set,
    /// yielding a bogus `InvalidColumnIndex` for the trailing columns.
    #[test]
    fn open_window_survives_a_column_drop() {
        let (db, _) = EmbeddedDb::open(":memory:").unwrap();
        db.execute(
            "CREATE TABLE t(id INTEGER PRIMARY KEY, a TEXT, b TEXT, note TEXT);
             INSERT INTO t(a,b,note) VALUES ('x', 'y', 'z');",
        )
        .unwrap();
        // First open prepares and caches `SELECT rowid, *, count(*) …`.
        let (page, _) = db.open_window("t", 0, 10).unwrap();
        assert_eq!(page.rows[0].len(), 4);
        db.execute("ALTER TABLE t DROP COLUMN note").unwrap();
        // Second open reuses the cached statement: the row is narrower.
        let (page, total) = db.open_window("t", 0, 10).unwrap();
        assert_eq!(total, 1);
        assert_eq!(page.rows[0].len(), 3);
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
            // Non-finite reals render with letters — pin the slow path.
            PValue::Real(f64::NAN),
            PValue::Real(f64::INFINITY),
            PValue::Real(f64::NEG_INFINITY),
            PValue::Null,
            PValue::Blob(vec![0xab, 0x12]),
        ];
        for needle in [
            "ada", "LACE", "42", "100", "∅", "x'ab", "zzz", "", "nan", "inf", "-inf",
        ] {
            let lc = needle.to_ascii_lowercase();
            let has_alpha = lc.bytes().any(|b| b.is_ascii_alphabetic());
            for v in &cases {
                let old = v.render().to_ascii_lowercase().contains(&lc);
                assert_eq!(v.contains_ci(&lc, has_alpha), old, "{v:?} vs {needle:?}");
            }
        }
    }

    /// Both backends (and every response shape) cross the worker
    /// thread boundary: prove Send at compile time.
    #[test]
    fn backends_and_responses_are_send() {
        fn assert_send<T: Send>() {}
        assert_send::<EmbeddedDb>();
        assert_send::<crate::remote::RemoteDb>();
        assert_send::<PValue>();
        assert_send::<TableInfo>();
        assert_send::<ColumnInfo>();
        assert_send::<Page>();
        assert_send::<QueryResult>();
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
        assert_eq!(
            links,
            [(
                "orders".to_owned(),
                "customer_id".to_owned(),
                "id".to_owned()
            )]
        );
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

//! DbWorker — the worker thread that owns the DbLink (DESIGN.md).
//!
//! Slice 1: synchronous parity. The UI thread holds a [`DbHandle`]
//! that implements [`DbLink`] by round-tripping jobs to the worker and
//! blocking for the answer, so every existing call site behaves EXACTLY
//! as before (plus a thread hop of microseconds). Later slices submit
//! async jobs for hot paths and reconcile responses as Commands; the
//! protocol (tokens, [`DbResponse`], submit/poll) already supports it.
//!
//! Design notes:
//! - The worker OWNS the link; `&dyn DbLink` never leaves its thread,
//!   so the backends' Mutexes are pure Send-enablers, never contended.
//! - Jobs are `FnMut` closures over owned captures (object-safe, Send).
//! - The handle uses interior mutability (RefCell/Cell) so it can
//!   implement `DbLink` (`&self` methods). It never crosses threads,
//!   so this is sound — only the link moves.
//! - A dead worker surfaces as [`DbResponse::Gone`], never a hang:
//!   blocking calls return it, the UI shows it as a status error.

use std::cell::{Cell, RefCell};
use std::sync::mpsc;
use std::time::Duration;

use crate::db::{ColumnInfo, DbLink, DbResult, PValue, Page, QueryResult, TableInfo};

pub type Token = u64;

/// Every result shape the worker can send back. Debug+Clone+PartialEq:
/// responses ride Commands in later slices.
#[derive(Debug, Clone, PartialEq)]
pub enum DbResponse {
    /// The worker is gone (panic or closed channel). Callers surface it.
    Gone,
    Tables(DbResult<Vec<TableInfo>>),
    Columns(DbResult<Vec<ColumnInfo>>),
    Count(DbResult<i64>),
    HasRowid(bool),
    RowidColumn(DbResult<Option<String>>),
    Page(DbResult<Page>),
    Window(DbResult<(Page, i64)>),
    Query(DbResult<QueryResult>),
    Execute(DbResult<(i64, Duration)>),
    Unit(DbResult<()>),
    Insert(DbResult<i64>),
    Health(Option<String>),
    Links(Vec<(String, String, String)>),
    /// Bundled table open: columns + rowid-ness + first window + total,
    /// fetched as one job so cold opens cost a single round-trip.
    Opened(DbResult<OpenedGrid>),
    /// Bundled health console: base + report + sparks + dot in one job.
    HealthConsole(DbResult<HealthData>),
    /// Split-view detail pane: filtered child rows + total in one job.
    Detail(DbResult<DetailData>),
}

/// Everything open_table needs to build a Grid, from one worker job.
#[derive(Debug, Clone, PartialEq)]
pub struct OpenedGrid {
    pub columns: Vec<ColumnInfo>,
    pub editable: bool,
    pub page: Page,
    pub total: i64,
}

/// Everything the health console needs to render, from one worker job.
#[derive(Debug, Clone, PartialEq)]
pub struct HealthData {
    pub base: String,
    pub report: Vec<[String; 4]>,
    pub sparks: Vec<(String, Vec<f64>, String)>,
    pub health: Option<String>,
}

/// Split-view detail pane: the child rows of one parent record plus
/// the filtered total (count runs in the same job = one round-trip).
#[derive(Debug, Clone, PartialEq)]
pub struct DetailData {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<PValue>>,
    pub total: i64,
}

impl DbResponse {
    fn mismatch<T>(what: &str) -> DbResult<T> {
        Err(format!("db worker protocol mismatch in {what}"))
    }

    fn tables(self) -> DbResult<Vec<TableInfo>> {
        match self {
            DbResponse::Tables(r) => r,
            _ => Self::mismatch("tables"),
        }
    }

    fn columns(self) -> DbResult<Vec<ColumnInfo>> {
        match self {
            DbResponse::Columns(r) => r,
            _ => Self::mismatch("columns"),
        }
    }

    fn count(self) -> DbResult<i64> {
        match self {
            DbResponse::Count(r) => r,
            _ => Self::mismatch("count"),
        }
    }

    fn page(self) -> DbResult<Page> {
        match self {
            DbResponse::Page(r) => r,
            _ => Self::mismatch("page"),
        }
    }

    fn window(self) -> DbResult<(Page, i64)> {
        match self {
            DbResponse::Window(r) => r,
            _ => Self::mismatch("open_window"),
        }
    }

    fn query(self) -> DbResult<QueryResult> {
        match self {
            DbResponse::Query(r) => r,
            _ => Self::mismatch("query"),
        }
    }

    fn execute(self) -> DbResult<(i64, Duration)> {
        match self {
            DbResponse::Execute(r) => r,
            _ => Self::mismatch("execute"),
        }
    }

    fn unit(self) -> DbResult<()> {
        match self {
            DbResponse::Unit(r) => r,
            _ => Self::mismatch("unit"),
        }
    }

    fn insert(self) -> DbResult<i64> {
        match self {
            DbResponse::Insert(r) => r,
            _ => Self::mismatch("insert"),
        }
    }
}

/// Work a job performs on the link. FnMut (not FnOnce) so the worker
/// can invoke it through the box on stable Rust.
pub type Work = Box<dyn FnMut(&dyn DbLink) -> DbResponse + Send>;

struct Job {
    tag: Token,
    work: Work,
}

/// The UI thread's end of the worker. Single owner (App), never shared
/// across threads — hence RefCell/Cell interior mutability.
pub struct DbHandle {
    backend: &'static str,
    readonly: bool,
    display: String,
    tx: mpsc::Sender<Job>,
    rx: mpsc::Receiver<(Token, std::time::Duration, DbResponse)>,
    /// Responses that arrived while a blocking call waited for its own
    /// tag (only matters once async submits exist).
    buffer: RefCell<Vec<(Token, std::time::Duration, DbResponse)>>,
    next: Cell<Token>,
}

/// Spawn the worker owning `link`; returns the UI-side handle.
pub fn spawn(link: Box<dyn DbLink>) -> DbHandle {
    let backend = link.backend();
    let readonly = link.readonly();
    let display = link.name().to_owned();
    let (job_tx, job_rx) = mpsc::channel::<Job>();
    let (res_tx, res_rx) = mpsc::channel::<(Token, std::time::Duration, DbResponse)>();
    std::thread::Builder::new()
        .name("phosphor-db".into())
        .spawn(move || {
            for job in job_rx {
                let mut work = job.work;
                let t0 = std::time::Instant::now();
                let resp = work(&*link);
                // The WORK duration, measured on the worker thread.
                // (Measuring submit-to-arrival on the UI side would
                // include poll idle time — the "everything takes
                // 250ms" bug.)
                let took = t0.elapsed();
                if res_tx.send((job.tag, took, resp)).is_err() {
                    break; // UI gone; exit
                }
            }
        })
        .expect("db worker thread failed to spawn");
    DbHandle {
        backend,
        readonly,
        display,
        tx: job_tx,
        rx: res_rx,
        buffer: RefCell::new(Vec::new()),
        next: Cell::new(1),
    }
}

impl DbHandle {
    /// `&dyn DbLink` view for APIs that take the trait directly
    /// (designer loaders, report renders, store helpers).
    pub fn link(&self) -> &dyn DbLink {
        self
    }

    fn alloc(&self) -> Token {
        let t = self.next.get();
        self.next.set(t + 1);
        t
    }

    /// Run `work` on the worker and block for its response.
    pub fn call(&self, work: Work) -> DbResponse {
        let tag = self.alloc();
        if self.tx.send(Job { tag, work }).is_err() {
            return DbResponse::Gone;
        }
        loop {
            // Position computed under a short borrow (scrutinee
            // temporaries in `if let` live through the block — never
            // overlap the borrow_mut below).
            let hit = self.buffer.borrow().iter().position(|(t, _, _)| *t == tag);
            if let Some(i) = hit {
                return self.buffer.borrow_mut().remove(i).2;
            }
            match self.rx.recv() {
                Ok((t, took, r)) => {
                    if t == tag {
                        return r;
                    }
                    self.buffer.borrow_mut().push((t, took, r));
                }
                Err(_) => return DbResponse::Gone,
            }
        }
    }

    /// Fire-and-forget for later slices; responses surface via poll().
    /// Returns None when the worker is gone.
    #[allow(dead_code)] // slice 1 is blocking-parity; async slices use this
    pub fn submit(&self, work: Work) -> Option<Token> {
        let tag = self.alloc();
        self.tx.send(Job { tag, work }).ok()?;
        Some(tag)
    }

    /// Drain all arrived async responses (plus anything buffered).
    #[allow(dead_code)] // slice 1 is blocking-parity; async slices use this
    pub fn poll(&self) -> Vec<(Token, std::time::Duration, DbResponse)> {
        while let Ok(msg) = self.rx.try_recv() {
            self.buffer.borrow_mut().push(msg);
        }
        std::mem::take(&mut *self.buffer.borrow_mut())
    }
}

/// Blocking DbLink façade over the worker: every method round-trips.
/// Slice-1 behavior is identical to direct calls; later slices bypass
/// this façade for hot paths via submit/poll.
impl DbLink for DbHandle {
    fn readonly(&self) -> bool {
        self.readonly
    }
    fn backend(&self) -> &'static str {
        self.backend
    }

    fn name(&self) -> &str {
        &self.display
    }

    fn tables(&self) -> DbResult<Vec<TableInfo>> {
        self.call(Box::new(|db| DbResponse::Tables(db.tables())))
            .tables()
    }

    fn columns(&self, table: &str) -> DbResult<Vec<ColumnInfo>> {
        let t = table.to_owned();
        self.call(Box::new(move |db| DbResponse::Columns(db.columns(&t))))
            .columns()
    }

    fn count(&self, table: &str) -> DbResult<i64> {
        let t = table.to_owned();
        self.call(Box::new(move |db| DbResponse::Count(db.count(&t))))
            .count()
    }

    fn has_rowid(&self, table: &str) -> bool {
        let t = table.to_owned();
        match self.call(Box::new(move |db| DbResponse::HasRowid(db.has_rowid(&t)))) {
            DbResponse::HasRowid(b) => b,
            _ => false,
        }
    }

    fn rowid_column(&self, table: &str) -> DbResult<Option<String>> {
        let t = table.to_owned();
        match self.call(Box::new(move |db| {
            DbResponse::RowidColumn(db.rowid_column(&t))
        })) {
            DbResponse::RowidColumn(r) => r,
            _ => Err("database worker did not return row identity".into()),
        }
    }

    fn page(&self, table: &str, offset: i64, limit: i64) -> DbResult<Page> {
        let t = table.to_owned();
        self.call(Box::new(move |db| {
            DbResponse::Page(db.page(&t, offset, limit))
        }))
        .page()
    }

    fn open_window(&self, table: &str, offset: i64, limit: i64) -> DbResult<(Page, i64)> {
        let t = table.to_owned();
        self.call(Box::new(move |db| {
            DbResponse::Window(db.open_window(&t, offset, limit))
        }))
        .window()
    }

    fn query(&self, sql: &str) -> DbResult<QueryResult> {
        let s = sql.to_owned();
        self.call(Box::new(move |db| DbResponse::Query(db.query(&s))))
            .query()
    }

    fn apply_schema_changes(&self, statements: &[String]) -> DbResult<Duration> {
        let statements = statements.to_owned();
        self.call(Box::new(move |db| {
            DbResponse::Execute(
                db.apply_schema_changes(&statements)
                    .map(|elapsed| (0, elapsed)),
            )
        }))
        .execute()
        .map(|(_, elapsed)| elapsed)
    }

    fn execute_params(&self, sql: &str, params: &[PValue]) -> DbResult<(i64, Duration)> {
        let sql = sql.to_owned();
        let params = params.to_owned();
        self.call(Box::new(move |db| {
            DbResponse::Execute(db.execute_params(&sql, &params))
        }))
        .execute()
    }

    fn execute(&self, sql: &str) -> DbResult<(i64, Duration)> {
        let s = sql.to_owned();
        self.call(Box::new(move |db| DbResponse::Execute(db.execute(&s))))
            .execute()
    }

    fn update_row(&self, table: &str, rowid: i64, changes: &[(String, PValue)]) -> DbResult<()> {
        let (t, c) = (table.to_owned(), changes.to_vec());
        self.call(Box::new(move |db| {
            DbResponse::Unit(db.update_row(&t, rowid, &c))
        }))
        .unit()
    }

    fn insert_row(&self, table: &str, changes: &[(String, PValue)]) -> DbResult<i64> {
        let (t, c) = (table.to_owned(), changes.to_vec());
        self.call(Box::new(move |db| {
            DbResponse::Insert(db.insert_row(&t, &c))
        }))
        .insert()
    }

    fn delete_row(&self, table: &str, rowid: i64) -> DbResult<()> {
        let t = table.to_owned();
        self.call(Box::new(move |db| {
            DbResponse::Unit(db.delete_row(&t, rowid))
        }))
        .unit()
    }

    fn health(&self) -> Option<String> {
        match self.call(Box::new(|db| DbResponse::Health(db.health()))) {
            DbResponse::Health(h) => h,
            _ => None,
        }
    }

    fn child_links(&self, parent: &str) -> Vec<(String, String, String)> {
        let p = parent.to_owned();
        match self.call(Box::new(move |db| DbResponse::Links(db.child_links(&p)))) {
            DbResponse::Links(l) => l,
            _ => Vec::new(),
        }
    }
}

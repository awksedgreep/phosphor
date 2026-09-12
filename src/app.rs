//! App state + the command bus (DESIGN.md, scripting-ready rule 1).
//!
//! Keys become `Command`s; `apply` is the ONLY place state changes. A
//! future script emits the same commands through the same function and
//! inherits every behavior and check for free.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use std::collections::{BTreeMap, HashMap};

use crate::appsgen::{self, ActionKind, AppDesignState, AppItem, AppMenuState};
use crate::creator::CreateState;
use crate::db::{ColumnInfo, DbLink, DbResult, PValue, TableInfo};
use crate::forms::{apply_mask, mask_ok, BoxItem, FormSpec, FormState, PaintState, TextItem};
use crate::help::{self, HelpState};
use crate::qbe::{QbeSpec, QbeState};
use crate::report::{self, PagerState, ReportSpec, ReportState};
use crate::store;
use crate::theme::{self, Theme};
use crate::worker::{DbHandle, DbResponse};

const OVERSCAN: i64 = 64;
const WIDTH_SAMPLE: usize = 50;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Focus {
    Sidebar,
    Grid,
    /// The detail pane of a split BROWSE (SET RELATION on one screen).
    Detail,
    Prompt,
}

// The design states differ widely in size (a full EDIT form vs a small
// help cursor); they live one-at-a-time in one App field, so the enum
// being as large as its biggest variant is fine.
#[allow(clippy::large_enum_variant)]
pub enum Overlay {
    None,
    Help(HelpState),
    Edit(EditState),
    Health(HealthView),
    Qbe(QbeState),
    Report(ReportState),
    Pager(PagerState),
    Form(FormState),
    Paint(PaintState),
    Create(CreateState),
    Apps(AppDesignState),
    AppMenu(AppMenuState),
    /// The multi-line Lua editor for a form lifecycle script.
    ScriptEditor(ScriptState),
}

/// What the script editor is bound to: a form lifecycle event, or the
/// one-line action of an application menu item.
#[derive(Clone)]
pub enum ScriptTarget {
    Form {
        table: String,
        event: String,
    },
    MenuItem {
        app: String,
        item_id: i64,
        label: String,
    },
}

/// A full-screen text buffer for one script.
pub struct ScriptState {
    pub target: ScriptTarget,
    pub lines: Vec<String>,
    /// Line index and char index (not bytes) of the caret.
    pub row: usize,
    pub col: usize,
    pub dirty: bool,
}

impl ScriptState {
    pub fn new(target: ScriptTarget, source: &str) -> Self {
        let mut lines: Vec<String> = source.split('\n').map(str::to_owned).collect();
        if lines.is_empty() {
            lines.push(String::new());
        }
        ScriptState {
            target,
            lines,
            row: 0,
            col: 0,
            dirty: false,
        }
    }

    pub fn text(&self) -> String {
        self.lines.join("\n")
    }

    /// The box title: what this script is bound to.
    pub fn title(&self) -> String {
        match &self.target {
            ScriptTarget::Form { table, event } => format!(" SCRIPT · {table} · {event}"),
            ScriptTarget::MenuItem { app, label, .. } => {
                format!(" SCRIPT · {app} · {label}")
            }
        }
    }

    fn line_len(&self) -> usize {
        self.lines[self.row].chars().count()
    }
}

/// The byte offset of character index `n` (clamped to the end).
fn byte_at(s: &str, n: usize) -> usize {
    s.char_indices().nth(n).map_or(s.len(), |(b, _)| b)
}

/// Phase 3: the dbhealth console — the report rendered as a system
/// screen, with sparklines fed straight from the compressed series.
pub struct HealthView {
    /// The dbhealth vtab name (report view minus `_report`).
    pub table: String,
    /// (check, status, value, advice) rows, worst-first (view order).
    pub report: Vec<[String; 4]>,
    /// (series name, recent values oldest→newest, latest rendered).
    pub sparks: Vec<(String, Vec<f64>, String)>,
}

/// A related-child pane under the EDIT form: the SET RELATION of
/// 1988, discovered from declared foreign keys.
#[derive(Clone)]
pub struct LinkPane {
    pub child: String,
    pub child_col: String,
    /// Rendered preview rows (already stringified, first few columns).
    pub header: Vec<String>,
    pub rows: Vec<Vec<String>>,
    pub total: i64,
    /// The parent-side value this pane is filtered on, as a SQL literal.
    pub key_sql: String,
}

pub struct EditState {
    pub table: String,
    /// true → this is a NEW record (INSERT on save; rowid unused).
    pub inserting: bool,
    /// The painted layout, when the crafted form has 2D coordinates —
    /// draw_edit renders CREATE SCREEN style instead of the list.
    pub painted: Option<FormSpec>,
    /// Absolute grid row of this record (paging context; 0 for NEW).
    pub row_abs: i64,
    pub rowid: i64,
    pub fields: Vec<(ColumnInfo, PValue)>,
    /// Display labels (custom when a crafted form exists for the table).
    pub labels: Vec<String>,
    /// Required-ness per field (from the crafted form; save enforces).
    pub required: Vec<bool>,
    /// PICTURE mask per field (from the crafted form; empty = free text).
    pub masks: Vec<String>,
    /// Computed expression per field; Some = read-only calculated value.
    pub computed: Vec<Option<String>>,
    /// Edited text per field; None = untouched.
    pub inputs: Vec<Option<String>>,
    pub cursor: usize,
    /// Some(buffer) while a field is being typed into.
    pub editing: Option<String>,
    /// Child panes from declared FKs pointing at this table.
    pub links: Vec<LinkPane>,
    /// (parent table, parent key column) per field when the column is a
    /// declared outgoing FK — the target of `F7` value lookup.
    pub pickers: Vec<Option<(String, String)>>,
    /// The open FK picker, if any (drawn over the form).
    pub picker: Option<PickerState>,
}

impl EditState {
    pub fn dirty(&self) -> bool {
        self.inputs.iter().any(Option::is_some)
    }
}

/// A value-lookup pop-up for a foreign-key field (dBASE-style `F7`):
/// pick a parent row and the key value is written into the field.
pub struct PickerState {
    pub title: String,
    pub columns: Vec<String>,
    pub rows: Vec<Vec<PValue>>,
    pub cursor: usize,
    /// Index of the EDIT field this picker fills.
    pub field: usize,
    /// Index of the key column within `columns`.
    pub key_col: usize,
}

pub enum GridSource {
    Table {
        name: String,
        editable: bool,
    },
    Query {
        truncated: bool,
    },
    /// Split-view detail pane: the child rows of one parent record,
    /// fetched in full (bounded) — read-only in v1.
    Detail {
        parent: String,
        child: String,
        child_col: String,
        key_sql: String,
    },
}

/// Screen regions for mouse hit-testing (content rects, borders
/// excluded), refreshed by the renderer every frame.
#[derive(Default, Clone, Copy)]
pub struct HitRects {
    pub sidebar: Option<ratatui::layout::Rect>,
    pub master: Option<ratatui::layout::Rect>,
    pub detail: Option<ratatui::layout::Rect>,
    pub prompt: Option<ratatui::layout::Rect>,
}

/// The right-hand pane of a split BROWSE: one child table filtered to
/// the master cursor's record. SET RELATION, on one screen. The pane's
/// Grid is read-only (GridSource::Detail); child/col/key live in the
/// source — one copy of the truth.
pub struct DetailState {
    pub grid: Grid,
    /// Parent-side FK column name ("" = the parent's pk / rowid).
    pub parent_col: String,
    /// Viewport height, reported back by the renderer each frame.
    pub visible_rows: i64,
    /// The 'v' cycle order, FROZEN when the pane opened (remembered
    /// link first). Cycling rewrites the pref, so recomputing the
    /// order per press would bounce between the first two links and
    /// never reach the rest — caught by the CRM demo.
    pub cycle: Vec<(String, String, String)>,
}

pub struct Grid {
    pub source: GridSource,
    pub columns: Vec<String>,
    pub total: i64,
    /// Cached rows; for Query sources this is ALL rows (cache_start 0).
    pub cache: Vec<Vec<PValue>>,
    pub cache_start: i64,
    pub rowids: Option<Vec<i64>>,
    pub cur_row: i64,
    pub cur_col: usize,
    pub row_off: i64,
    pub col_off: usize,
    pub widths: Vec<u16>,
}

impl Grid {
    pub fn row(&self, abs: i64) -> Option<&Vec<PValue>> {
        let idx = abs.checked_sub(self.cache_start)?;
        self.cache.get(idx as usize)
    }

    fn compute_widths(&mut self) {
        self.widths = self
            .columns
            .iter()
            .enumerate()
            .map(|(c, name)| {
                let mut w = name.chars().count();
                for row in self.cache.iter().take(WIDTH_SAMPLE) {
                    if let Some(v) = row.get(c) {
                        // render_len, not render: no String per cell.
                        w = w.max(v.render_len());
                    }
                }
                w.clamp(4, 24) as u16
            })
            .collect();
    }
}

pub struct Prompt {
    pub input: String,
    pub cursor: usize,
    pub history: Vec<String>,
    hist_pos: Option<usize>,
}

/// The command bus. Everything a user (or someday a script) can do.
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    Quit,
    /// Begin editing the current EDIT-form field with this first char.
    EditType(char),
    /// Begin editing the current designer NAME cell with this char.
    CreateType(char),
    /// Edit the REFERENCES target of the current designer field.
    CreateRefs,
    /// Open link pane N as a filtered BROWSE of the child table.
    EditOpenLink(usize),
    /// F7 on a foreign-key field: open the parent-row value picker.
    EditPick,
    /// Move the open picker's cursor.
    PickerMove(i64),
    /// Write the picked key value into the field and close.
    PickerCommit,
    /// Close the picker without changing the field.
    PickerCancel,
    Focus(Focus),
    Back,
    Help,
    Refresh,
    SidebarMove(i64),
    OpenSelected,
    GridMove {
        dr: i64,
        dc: i64,
    },
    GridPage(i64),
    GridEdge(bool),
    GridTop,
    GridBottom,
    OpenEdit,
    EditMove(i64),
    EditBegin,
    EditChar(char),
    EditBackspace,
    EditCommitField,
    EditSave,
    PromptChar(char),
    PromptBackspace,
    PromptMove(i64),
    PromptHistory(i64),
    PromptRun,
    OpenHealth,
    HealthSample,
    // Generic designer commands (QBE, report — later forms/apps): each
    // overlay interprets them per its own semantics. One bus, always.
    OpenQbe(Option<String>),
    OpenReport(Option<String>),
    OpenLabels(Option<String>),
    DesignerMove(i64),
    DesignerToggle,
    DesignerCycle,
    DesignerEditBegin,
    DesignerChar(char),
    DesignerBackspace,
    DesignerCommit,
    DesignerRun,
    DesignerSave,
    DesignerAdd,
    DesignerDelete,
    DesignerSwap(i64),
    DesignerEditAlt,
    /// Forms: edit the selected field's PICTURE mask (`m`).
    DesignerEditMask,
    /// Forms: edit the selected field's computed expression (`c`).
    DesignerEditComputed,
    /// QBE: cycle the FK-driven JOIN on/off (`J`).
    DesignerJoin,
    /// QBE: cycle the GROUP BY column on/off (`g`).
    DesignerGroup,
    PagerScroll(i64),
    PagerWrite,
    PagerPrint,
    OpenForm(Option<String>),
    /// Script effects (rule 5): open a named table / run a saved query /
    /// render a saved report, mapped from `ui.browse/query/report`.
    OpenTable(String),
    OpenSavedQuery(String),
    OpenSavedReport(String),
    /// Open the multi-line Lua editor for a form lifecycle script.
    OpenFormScript {
        table: String,
        event: String,
    },
    /// Open the multi-line Lua editor for the selected menu item's script.
    OpenSelectedScript,
    /// Applications Generator: rename the current app (`r`).
    RenameApp,
    ScriptChar(char),
    ScriptTab,
    ScriptNewline,
    ScriptBackspace,
    ScriptMove {
        dl: i64,
        dc: i64,
    },
    /// Move the caret to the start (false) or end (true) of the line.
    ScriptLineEdge(bool),
    ScriptSave,
    OpenApps(Option<String>),
    OpenAppMenu(Option<String>),
    OpenInsert,
    /// Flip the EDIT form to the previous/next RECORD (dBASE paging).
    EditPage(i64),
    DeleteRow,
    FindNext,
    PromptClear,
    PromptDeleteWord,
    PromptComplete,
    PaintMove {
        dx: i64,
        dy: i64,
    },
    PaintPlace,
    PaintBox,
    PaintText,
    PaintDelete,
    HelpScroll(i64),
    HelpTopic(i64),
    /// The TABLE DESIGNER (dBASE CREATE structure screen).
    OpenCreate(Option<String>),
    CreatePk,
    CreateNull,
    CreateUnique,
    /// First-letter seek in the sidebar (dBASE-style, cycling).
    SidebarSeek(char),
    /// Show/hide internal tables (shadow, _phosphor, dbhealth views).
    ToggleInternals,
    /// Toggle the split-view detail pane (SET RELATION on one screen).
    /// Cycles among a table's related children; 'v' again closes.
    ToggleSplit,
    /// Flip the split orientation between side-by-side and stacked ('H').
    ToggleSplitDir,
    /// The TABLE EDITOR: open the selected table's structure for
    /// changes (add / rename / drop columns), applied as ALTERs.
    OpenTableEditor,
    // Mouse equivalents (hit-testing happens in App::on_mouse):
    /// Select the Nth visible sidebar row; clicking the selection
    /// again opens it.
    SidebarClick(usize),
    /// Place the focused pane's cursor on absolute row N; clicking
    /// the master's selection again opens EDIT.
    GridClick {
        row: i64,
    },
    /// Scroll the pane under the wheel by N rows.
    GridScroll(i64),
    /// A worker response arrived (main loop polls, tests pump): the
    /// token routes it to its pending continuation in finish_db().
    DbReady(
        crate::worker::Token,
        std::time::Duration,
        crate::worker::DbResponse,
    ),
}

/// An outstanding async worker job and what to do with its answer.
/// The single worker answers FIFO, so arrivals install in submit
/// order and converge on the latest state without generation guards.
enum PendingOp {
    /// Table open: build + swap in a fresh grid on arrival.
    Open { name: String, seq: u64 },
    /// Ad-hoc SELECT: build a query grid on arrival.
    Select { seq: u64 },
    /// Refresh: total + window into the live grid, then re-seek.
    Refill {
        name: String,
        row: i64,
        col: usize,
        want_start: i64,
    },
    /// Scroll window: installs into the live grid when still wanted.
    Page { table: String, want_start: i64 },
    /// Find scan: one table window per response; hits jump, misses
    /// chain the next window until the cap. Superseded scans die by seq.
    Find {
        table: String,
        needle: String,
        needle_lc: String,
        needle_has_alpha: bool,
        offset: i64,
        end: i64,
        seq: u64,
        /// Status epoch at submit: completion messages only overwrite
        /// what was on screen when the scan started.
        sseq: u64,
    },
    /// Health console rebuild: installs only if the overlay hasn't
    /// moved on (discriminant guard); `sampled` says so on arrival.
    Health {
        sampled: bool,
        seq: u64,
        overlay: std::mem::Discriminant<Overlay>,
    },
    /// Detail-pane rows for a split BROWSE, keyed by the parent key
    /// they were fetched for (stale arrivals drop).
    Detail { want_key: String },
}

pub struct App {
    /// The database, behind the worker thread (worker.rs). All calls
    /// round-trip and block in slice 1 — identical behavior, and the
    /// façade keeps every call site unchanged for later async slices.
    pub db: DbHandle,
    /// Set by `--app`: Esc at top level returns to this app's menu.
    pub app_home: Option<String>,
    /// `--app --readonly`: a kiosk that browses and runs reports but
    /// refuses every write. Set by main before the loop starts.
    pub readonly: bool,
    /// Split orientation: false = side by side (default), true = master
    /// stacked above the detail pane (`H` toggles; remembered).
    pub split_horizontal: bool,
    pub theme: &'static Theme,
    /// CRT scanline affectation (DESIGN.md): dims every other screen
    /// row. Off by default; persisted per database.
    pub shimmer: bool,
    pub focus: Focus,
    pub overlay: Overlay,
    pub tables: Vec<TableInfo>,
    pub sidebar_idx: usize,
    pub grid: Option<Grid>,
    /// Split-view detail pane (None = single-pane BROWSE, as always).
    /// The pane's Grid lives here too; its source is GridSource::Detail.
    pub detail: Option<DetailState>,
    pub prompt: Prompt,
    /// (message, is_error) for the status line.
    pub status: Option<(String, bool)>,
    pub last_ms: Option<f64>,
    pub health: Option<String>,
    pub quit: bool,
    /// Grid viewport height, reported back by the renderer each frame.
    pub visible_rows: i64,
    pub visible_cols_width: u16,
    /// Armed delete: (table, rowid) — second 'x' on the same row fires.
    pending_delete: Option<(String, i64)>,
    /// Last automatic health sample (the console is LIVE while open).
    last_auto_sample: std::time::Instant,
    /// The last `find <text>` needle; 'n' repeats it.
    last_find: Option<String>,
    /// Sidebar shows internal tables (shadow/_phosphor/health views)?
    pub show_internals: bool,
    /// An overlay was launched from the app menu: Esc returns HOME.
    menu_launched: bool,
    /// The app whose designer launched the script editor; Esc returns.
    script_return_app: Option<String>,
    /// Select-all semantics for prefilled single-line editors: the
    /// first typed char REPLACES the prefill; Backspace edits it.
    editor_fresh: bool,
    /// Held-key acceleration state for record paging.
    last_edit_page: Option<std::time::Instant>,
    page_streak: u32,
    /// Dirty-flag redraw (main.rs): set by apply()/tick(), cleared by
    /// the main loop after drawing. The terminal is static between
    /// commands, so idle ticks skip the full redraw.
    pub dirty: bool,
    /// Status-bar health dot cache: health() is 2 round-trips (worse
    /// over sqld) and the dot is advisory, so opens/refreshes reuse a
    /// fresh-enough value. Writes invalidate; TTL bounds time drift.
    health_cache: Option<(Option<String>, std::time::Instant)>,
    /// Per-table caches so EDIT record flips don't re-query schema,
    /// re-parse the saved form, or re-run FK introspection per record.
    /// Cleared in reload_tables() (every schema-changing path funnels
    /// through it) and on form save.
    columns_cache: HashMap<String, Vec<ColumnInfo>>,
    form_cache: HashMap<String, Option<FormSpec>>,
    links_cache: HashMap<String, Vec<(String, String, String)>>,
    /// Outgoing FK targets per table — the F7 pickers' fuel.
    fks_cache: HashMap<String, Vec<(String, String, Option<String>)>>,
    /// Pane previews keyed by (child, child_col, key_sql): flipping
    /// back across records reuses them instead of re-querying.
    /// Cleared on writes (counts/previews may change) and capped.
    pane_cache: HashMap<(String, String, String), LinkPane>,
    /// Outstanding async worker jobs by token (finish_db routes answers).
    pending: HashMap<crate::worker::Token, PendingOp>,
    /// Latest in-flight scroll window (token, table, want_start): older
    /// arrivals drop, so hold-to-fly converges on the newest window.
    pending_page: Option<(crate::worker::Token, String, i64)>,
    /// Latest in-flight detail-pane fetch and the key it was for.
    pending_detail: Option<(crate::worker::Token, String)>,
    /// Screen regions for mouse hit-testing, written back by the
    /// renderer every frame (content rects, borders excluded).
    pub hit: HitRects,
    /// EDIT target parked while its window flies in (built on arrival).
    pending_edit: Option<i64>,
    /// Find-scan generation: a new find supersedes older chains.
    find_seq: u64,
    /// Status epoch: async completions only touch the status line when
    /// no newer message has landed since they were submitted.
    status_seq: u64,
}

impl App {
    pub fn new(db: Box<dyn DbLink>, warning: Option<String>) -> Self {
        let mut app = App {
            // The worker takes ownership of the connection here; every
            // db call below round-trips to it (slice 1: blocking parity).
            db: crate::worker::spawn(db),
            app_home: None,
            readonly: false,
            split_horizontal: false,
            theme: &theme::GREEN,
            shimmer: false,
            focus: Focus::Sidebar,
            overlay: Overlay::None,
            tables: Vec::new(),
            sidebar_idx: 0,
            grid: None,
            detail: None,
            prompt: Prompt {
                input: String::new(),
                cursor: 0,
                history: Vec::new(),
                hist_pos: None,
            },
            status: warning.map(|w| (w, true)),
            last_ms: None,
            health: None,
            quit: false,
            visible_rows: 20,
            visible_cols_width: 80,
            pending_delete: None,
            last_find: None,
            show_internals: false,
            menu_launched: false,
            script_return_app: None,
            editor_fresh: false,
            last_edit_page: None,
            page_streak: 0,
            last_auto_sample: std::time::Instant::now(),
            dirty: true, // first frame must paint
            health_cache: None,
            columns_cache: HashMap::new(),
            form_cache: HashMap::new(),
            links_cache: HashMap::new(),
            fks_cache: HashMap::new(),
            pane_cache: HashMap::new(),
            pending: HashMap::new(),
            pending_page: None,
            pending_detail: None,
            pending_edit: None,
            hit: HitRects::default(),
            find_seq: 0,
            status_seq: 0,
        };
        app.reload_tables();
        app.health = app.db.health();
        // Restore persisted appearance (read-only path: a database with
        // no prefs table simply keeps the defaults).
        if let Some(t) = store::pref_get(app.db.link(), "theme").and_then(|n| Theme::by_name(&n)) {
            app.theme = t;
        }
        app.shimmer = store::pref_get(app.db.link(), "shimmer").as_deref() == Some("on");
        app.split_horizontal = store::pref_get(app.db.link(), "split:dir").as_deref() == Some("h");
        app
    }

    fn say(&mut self, msg: impl Into<String>) {
        self.status_seq += 1;
        self.status = Some((msg.into(), false));
    }

    fn err(&mut self, msg: impl Into<String>) {
        self.status_seq += 1;
        self.status = Some((msg.into(), true));
    }

    /// Poll the worker for arrived async responses and apply each as a
    /// DbReady command. Non-blocking: the main loop calls this every
    /// iteration (unarrived responses apply on a later tick). Tests
    /// needing determinism use sync() instead.
    pub fn pump(&mut self) {
        for (tag, took, resp) in self.db.poll() {
            self.apply(Command::DbReady(tag, took, resp));
        }
    }

    /// Blocking drain for tests: parks (briefly) until every pending
    /// job has been answered and applied. Panics on timeout — a test
    /// must never outrun the worker silently.
    #[cfg(test)]
    pub fn sync(&mut self) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !self.pending.is_empty() {
            self.pump();
            if self.pending.is_empty() {
                return;
            }
            if std::time::Instant::now() > deadline {
                panic!(
                    "db worker did not answer {} pending job(s)",
                    self.pending.len()
                );
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }

    /// Route an arrived worker response to its pending continuation.
    /// Unknown tags are ignored (already handled — each tag resolves
    /// exactly once and is removed here).
    fn finish_db(
        &mut self,
        tag: crate::worker::Token,
        took: std::time::Duration,
        resp: DbResponse,
    ) {
        if matches!(resp, DbResponse::Gone) {
            self.pending.remove(&tag);
            self.err("database worker is gone");
            return;
        }
        let Some(op) = self.pending.remove(&tag) else {
            return;
        };
        match (op, resp) {
            (PendingOp::Open { name, seq }, DbResponse::Opened(r)) => match r {
                Ok(g) => {
                    // Refresh the schema caches the job already paid for.
                    self.columns_cache.insert(name.clone(), g.columns.clone());
                    let mut grid = Grid {
                        source: GridSource::Table {
                            name,
                            editable: g.editable,
                        },
                        columns: g.columns.iter().map(|c| c.name.clone()).collect(),
                        total: g.total,
                        cache: g.page.rows,
                        cache_start: 0,
                        rowids: g.page.rowids,
                        cur_row: 0,
                        cur_col: 0,
                        row_off: 0,
                        col_off: 0,
                        widths: Vec::new(),
                    };
                    grid.compute_widths();
                    self.grid = Some(grid);
                    self.focus = Focus::Grid;
                    self.pending_edit = None; // new table: parked rows are void
                    self.close_detail(); // the old pane links a different table
                    self.last_ms = Some(took.as_secs_f64() * 1000.0);
                    self.refresh_health();
                    // Clear only our own (silent) open — a newer message wins.
                    if self.status_seq == seq {
                        self.status = None;
                    }
                }
                Err(e) => self.err(e),
            },
            (PendingOp::Select { seq }, DbResponse::Query(r)) => match r {
                Ok(q) => {
                    let n = q.rows.len();
                    let truncated = q.truncated;
                    let mut grid = Grid {
                        source: GridSource::Query { truncated },
                        columns: q.columns,
                        total: n as i64,
                        cache: q.rows,
                        cache_start: 0,
                        rowids: None,
                        cur_row: 0,
                        cur_col: 0,
                        row_off: 0,
                        col_off: 0,
                        widths: Vec::new(),
                    };
                    grid.compute_widths();
                    self.grid = Some(grid);
                    self.focus = Focus::Grid;
                    self.pending_edit = None; // new result: parked rows are void
                    self.close_detail(); // query results have no child links
                    self.last_ms = Some(took.as_secs_f64() * 1000.0);
                    if self.status_seq == seq {
                        self.say(if truncated {
                            format!("{n} rows (capped) — add a WHERE or LIMIT")
                        } else {
                            format!("{n} row(s)")
                        });
                    }
                }
                Err(e) => self.err(e),
            },
            (
                PendingOp::Refill {
                    name,
                    row,
                    col,
                    want_start,
                },
                DbResponse::Window(r),
            ) => match r {
                Ok((page, total)) => {
                    let current = matches!(
                        &self.grid,
                        Some(g) if matches!(&g.source, GridSource::Table { name: n, .. } if n == &name)
                    );
                    if !current {
                        return; // user moved on; drop the stale window
                    }
                    if let Some(g) = &mut self.grid {
                        g.total = total;
                        g.cache = page.rows;
                        g.rowids = page.rowids;
                        g.cache_start = want_start;
                        g.cur_col = col.min(g.columns.len().saturating_sub(1));
                    }
                    self.last_ms = Some(took.as_secs_f64() * 1000.0);
                    self.grid_jump(row);
                    self.try_pending_edit();
                }
                Err(e) => self.err(e),
            },
            (PendingOp::Page { table, want_start }, DbResponse::Page(r)) => match r {
                Ok(page) => {
                    // Latest window wins; older arrivals drop.
                    if self.pending_page != Some((tag, table.clone(), want_start)) {
                        return;
                    }
                    self.pending_page = None;
                    let current = matches!(
                        &self.grid,
                        Some(g) if matches!(&g.source, GridSource::Table { name: n, .. } if n == &table)
                    );
                    if !current {
                        return; // grid moved on; drop the stale window
                    }
                    if let Some(g) = &mut self.grid {
                        g.cache = page.rows;
                        g.rowids = page.rowids;
                        g.cache_start = want_start;
                        if g.widths.is_empty() {
                            g.compute_widths();
                        }
                    }
                    self.last_ms = Some(took.as_secs_f64() * 1000.0);
                    self.try_pending_edit();
                }
                Err(e) => self.err(e),
            },
            (
                PendingOp::Find {
                    table,
                    needle,
                    needle_lc,
                    needle_has_alpha,
                    offset,
                    end,
                    seq,
                    sseq,
                },
                DbResponse::Page(r),
            ) => match r {
                Ok(page) => {
                    if seq != self.find_seq {
                        return; // superseded by a newer find; chain dies
                    }
                    let current = matches!(
                        &self.grid,
                        Some(g) if matches!(&g.source, GridSource::Table { name: n, .. } if n == &table)
                    );
                    if !current {
                        return; // user moved on; drop the stale window
                    }
                    for (i, row) in page.rows.iter().enumerate() {
                        if row
                            .iter()
                            .any(|v| v.contains_ci(&needle_lc, needle_has_alpha))
                        {
                            let abs = offset + i as i64;
                            self.grid_jump(abs);
                            self.focus = Focus::Grid;
                            if self.status_seq == sseq {
                                self.say(format!("found at row {}", abs + 1));
                            }
                            return;
                        }
                    }
                    let next = offset + page.rows.len() as i64;
                    if page.rows.is_empty() || next >= end {
                        if self.status_seq == sseq {
                            self.say(format!(
                                "{needle:?} not found below (g for top, n to retry)"
                            ));
                        }
                        return;
                    }
                    // Chain the next window (same scan generation).
                    let limit = Self::FIND_PAGE.min(end - next).max(0);
                    let op = PendingOp::Find {
                        table: table.clone(),
                        needle,
                        needle_lc,
                        needle_has_alpha,
                        offset: next,
                        end,
                        seq,
                        sseq,
                    };
                    match self.db.submit(Box::new(move |db| {
                        DbResponse::Page(db.page(&table, next, limit))
                    })) {
                        Some(tag) => {
                            self.pending.insert(tag, op);
                        }
                        None => self.err("database worker is gone"),
                    }
                }
                Err(e) => self.err(e),
            },
            (
                PendingOp::Health {
                    sampled,
                    seq,
                    overlay,
                },
                DbResponse::HealthConsole(r),
            ) => match r {
                Ok(h) => {
                    if std::mem::discriminant(&self.overlay) != overlay {
                        return; // user moved on; silent drop, no yank
                    }
                    self.health = h.health.clone();
                    self.health_cache = Some((h.health, std::time::Instant::now()));
                    self.last_auto_sample = std::time::Instant::now();
                    self.last_ms = Some(took.as_secs_f64() * 1000.0);
                    self.overlay = Overlay::Health(HealthView {
                        table: h.base,
                        report: h.report,
                        sparks: h.sparks,
                    });
                    if sampled && self.status_seq == seq {
                        self.say("sampled");
                    }
                }
                Err(e) => self.err(e),
            },
            (PendingOp::Detail { want_key }, DbResponse::Detail(r)) => match r {
                Ok(d) => {
                    // Latest fetch wins; older arrivals drop.
                    if self.pending_detail.as_ref() != Some(&(tag, want_key.clone())) {
                        return;
                    }
                    self.pending_detail = None;
                    let Some(state) = &mut self.detail else {
                        return;
                    };
                    let (parent, child, child_col) = match &state.grid.source {
                        GridSource::Detail {
                            parent,
                            child,
                            child_col,
                            ..
                        } => (parent.clone(), child.clone(), child_col.clone()),
                        _ => return,
                    };
                    state.grid.source = GridSource::Detail {
                        parent,
                        child,
                        child_col,
                        key_sql: want_key,
                    };
                    let g = &mut state.grid;
                    g.columns = d.columns;
                    g.total = d.total;
                    g.cache = d.rows;
                    g.cache_start = 0;
                    g.rowids = None;
                    g.cur_row = g.cur_row.clamp(0, g.total.saturating_sub(1).max(0));
                    g.row_off = g.row_off.min(g.cur_row);
                    g.compute_widths();
                    self.last_ms = Some(took.as_secs_f64() * 1000.0);
                }
                Err(e) => self.err(e),
            },
            (_, _) => self.err("db worker protocol mismatch"),
        }
    }

    /// Internal machinery a user did not create: phosphor's own
    /// catalog, engine shadow tables, and dbhealth's companion views.
    /// Heuristic by naming convention; the i toggle shows everything.
    pub fn is_internal(t: &TableInfo) -> bool {
        let n = t.name.as_str();
        n.starts_with('_')
            || [
                "_chunks",
                "_meta",
                "_series",
                "_blocks",
                "_terms",
                "_trace_blocks",
            ]
            .iter()
            .any(|suf| n.ends_with(suf))
            || (t.is_view
                && ["_report", "_now", "_trends"]
                    .iter()
                    .any(|s| n.ends_with(s)))
    }

    /// The tables the sidebar shows, honoring the internals toggle.
    pub fn visible_tables(&self) -> Vec<&TableInfo> {
        self.tables
            .iter()
            .filter(|t| self.show_internals || !Self::is_internal(t))
            .collect()
    }

    fn sidebar_seek(&mut self, c: char) {
        let visible = self.visible_tables();
        let n = visible.len();
        if n == 0 {
            return;
        }
        for step in 1..=n {
            let idx = (self.sidebar_idx + step) % n;
            if visible[idx]
                .name
                .chars()
                .next()
                .is_some_and(|f| f.eq_ignore_ascii_case(&c))
            {
                self.sidebar_idx = idx;
                return;
            }
        }
    }

    fn reload_tables(&mut self) {
        match self.db.tables() {
            Ok(t) => {
                self.tables = t;
                self.sidebar_idx = self
                    .sidebar_idx
                    .min(self.visible_tables().len().saturating_sub(1));
            }
            Err(e) => self.err(e),
        }
        // Schema may have changed: drop per-table caches (columns,
        // saved forms, FK links, pane previews) and the health dot.
        // Flips re-fill lazily.
        self.columns_cache.clear();
        self.form_cache.clear();
        self.links_cache.clear();
        self.fks_cache.clear();
        self.pane_cache.clear();
        self.health_cache = None;
        // A dropped/renamed browsed table would leave a zombie grid
        // (stale title + cached rows, quiet "no such table" refills):
        // close it and send the user back to the sidebar (issue #10).
        let gone = match &self.grid {
            Some(Grid {
                source: GridSource::Table { name, .. },
                ..
            }) => !self
                .tables
                .iter()
                .any(|t| t.name.eq_ignore_ascii_case(name)),
            _ => false,
        };
        if gone {
            self.grid = None;
            self.pending_page = None;
            self.close_detail();
            if matches!(self.focus, Focus::Grid | Focus::Detail) {
                self.focus = Focus::Sidebar;
            }
            self.say("the browsed table is gone — back to the table list");
        }
    }

    /// Health-dot TTL: re-query at most every 30 s between writes.
    const HEALTH_TTL: std::time::Duration = std::time::Duration::from_secs(30);

    /// Find-scan window: wide enough that round-trips (not matching)
    /// dominate scan cost; matching itself is zero-alloc.
    const FIND_PAGE: i64 = 4096;

    /// Advisory dot for opens/refreshes: cached value when fresh,
    /// one re-query otherwise. Console/sample paths query directly.
    fn refresh_health(&mut self) {
        let fresh = self
            .health_cache
            .as_ref()
            .is_some_and(|(_, t)| t.elapsed() < Self::HEALTH_TTL);
        if fresh {
            if let Some((h, _)) = &self.health_cache {
                self.health = h.clone();
            }
            return;
        }
        let h = self.db.health();
        self.health_cache = Some((h.clone(), std::time::Instant::now()));
        self.health = h;
    }

    /// Writes may move the dot: drop the cache (callers re-query).
    fn invalidate_health(&mut self) {
        self.health_cache = None;
    }

    /// Cached schema introspection for the EDIT hot path: one DB hit
    /// per table until the next reload_tables(), not one per record.
    fn cached_columns(&mut self, table: &str) -> crate::db::DbResult<Vec<ColumnInfo>> {
        if let Some(c) = self.columns_cache.get(table) {
            return Ok(c.clone());
        }
        let cols = self.db.columns(table)?;
        self.columns_cache.insert(table.to_owned(), cols.clone());
        Ok(cols)
    }

    /// Cached FormSpec::load: one parse per table, not one per record.
    fn cached_form(&mut self, table: &str) -> Option<FormSpec> {
        if let Some(s) = self.form_cache.get(table) {
            return s.clone();
        }
        let spec = FormSpec::load(self.db.link(), table);
        self.form_cache.insert(table.to_owned(), spec.clone());
        spec
    }

    /// Cached FK introspection: one (now batched) probe per table.
    fn cached_links(&mut self, parent: &str) -> Vec<(String, String, String)> {
        if let Some(l) = self.links_cache.get(parent) {
            return l.clone();
        }
        let links = self.db.child_links(parent);
        self.links_cache.insert(parent.to_owned(), links.clone());
        links
    }

    /// Cached outgoing-FK targets: one probe per table per schema epoch.
    fn cached_outgoing_fks(&mut self, table: &str) -> Vec<(String, String, Option<String>)> {
        if let Some(f) = self.fks_cache.get(table) {
            return f.clone();
        }
        let fks = self.db.outgoing_fks(table);
        self.fks_cache.insert(table.to_owned(), fks.clone());
        fks
    }

    // ── key → command (pure mapping; no state changes here) ──────────

    pub fn map_key(&self, key: KeyEvent) -> Option<Command> {
        use KeyCode::*;
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == Char('q') {
            return Some(Command::Quit);
        }
        if let Overlay::Edit(ed) = &self.overlay {
            // The FK picker, when open, owns the keyboard.
            if ed.picker.is_some() {
                return Some(match key.code {
                    Up | Char('k') => Command::PickerMove(-1),
                    Down | Char('j') => Command::PickerMove(1),
                    Enter => Command::PickerCommit,
                    Esc | Char('q') => Command::PickerCancel,
                    F(1) => Command::Help,
                    _ => return None,
                });
            }
            return Some(match (&ed.editing, key.code) {
                (Some(_), Enter) => Command::EditCommitField,
                (Some(_), Esc) => Command::Back,
                (Some(_), Backspace) => Command::EditBackspace,
                // Save WHILE typing a value: fold the buffer and save —
                // "type, F10" must work without an Enter in between
                // (field report: F10 appeared dead mid-edit).
                (Some(_), F(10)) => Command::EditSave,
                (Some(_), Char('s')) if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    Command::EditSave
                }
                (Some(_), PageDown) => Command::EditPage(1),
                (Some(_), PageUp) => Command::EditPage(-1),
                // Tab commits-and-advances like Enter; Shift-Tab folds
                // the buffer and steps back a field.
                (Some(_), Tab) => Command::EditCommitField,
                (Some(_), BackTab) => Command::EditMove(-1),
                (Some(_), Char(c)) => Command::EditChar(c),
                (None, Up | BackTab) => Command::EditMove(-1),
                (None, Down | Tab) => Command::EditMove(1),
                (None, PageDown | Right) => Command::EditPage(1),
                (None, PageUp | Left) => Command::EditPage(-1),
                (None, Enter) => Command::EditBegin,
                (None, F(10)) => Command::EditSave,
                (None, Char('s')) if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    Command::EditSave
                }
                (None, F(1)) => Command::Help,
                (None, F(4)) => Command::EditOpenLink(0),
                (None, F(5)) => Command::EditOpenLink(1),
                (None, F(6)) => Command::EditOpenLink(2),
                (None, F(7)) => Command::EditPick,
                (None, Esc) => Command::Back,
                // The form is LIVE, 1988-style: land on a field and
                // just type — no Enter required to begin.
                (None, Char(c))
                    if !key
                        .modifiers
                        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
                {
                    Command::EditType(c)
                }
                _ => return None,
            });
        }
        if matches!(self.overlay, Overlay::ScriptEditor(_)) {
            return Some(match key.code {
                Esc => Command::Back,
                Enter => Command::ScriptNewline,
                Backspace => Command::ScriptBackspace,
                Left => Command::ScriptMove { dl: 0, dc: -1 },
                Right => Command::ScriptMove { dl: 0, dc: 1 },
                Up => Command::ScriptMove { dl: -1, dc: 0 },
                Down => Command::ScriptMove { dl: 1, dc: 0 },
                PageUp => Command::ScriptMove { dl: -10, dc: 0 },
                PageDown => Command::ScriptMove { dl: 10, dc: 0 },
                Home => Command::ScriptLineEdge(false),
                End => Command::ScriptLineEdge(true),
                Tab => Command::ScriptTab,
                F(6) => Command::ScriptSave,
                F(1) => Command::Help,
                Char(c)
                    if !key
                        .modifiers
                        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
                {
                    Command::ScriptChar(c)
                }
                _ => return None,
            });
        }
        if matches!(self.overlay, Overlay::Help(_)) {
            return Some(match key.code {
                Esc | Char('q') | F(1) => Command::Back,
                Up | Char('k') => Command::HelpScroll(-1),
                Down | Char('j') => Command::HelpScroll(1),
                PageUp => Command::HelpScroll(-20),
                PageDown | Char(' ') => Command::HelpScroll(20),
                Home | Char('g') => Command::HelpScroll(i64::MIN / 2),
                Left | Char('h') => Command::HelpTopic(-1),
                Right | Char('l') | Tab => Command::HelpTopic(1),
                _ => return None,
            });
        }
        if matches!(self.overlay, Overlay::Health(_)) {
            return Some(match key.code {
                Esc | Char('q') | F(10) => Command::Back,
                Char('s') => Command::HealthSample,
                Char('r') | F(5) => Command::OpenHealth,
                F(1) => Command::Help,
                _ => return None,
            });
        }
        if let Overlay::Qbe(st) = &self.overlay {
            return Some(match (&st.editing, key.code) {
                (Some(_), Enter) => Command::DesignerCommit,
                (Some(_), Esc) => Command::Back,
                (Some(_), Backspace) => Command::DesignerBackspace,
                (Some(_), Char(c)) => Command::DesignerChar(c),
                (None, Up | Char('k')) => Command::DesignerMove(-1),
                (None, Down | Char('j')) => Command::DesignerMove(1),
                (None, Char(' ')) => Command::DesignerToggle,
                (None, Char('s')) => Command::DesignerCycle,
                (None, Char('J')) => Command::DesignerJoin,
                (None, Char('g')) => Command::DesignerGroup,
                (None, Enter) => Command::DesignerEditBegin,
                (None, F(2)) => Command::DesignerRun,
                (None, F(6)) => Command::DesignerSave,
                (None, F(1)) => Command::Help,
                (None, Esc) => Command::Back,
                _ => return None,
            });
        }
        if let Overlay::Report(st) = &self.overlay {
            return Some(match (&st.editing, key.code) {
                (Some(_), Enter) => Command::DesignerCommit,
                (Some(_), Esc) => Command::Back,
                (Some(_), Backspace) => Command::DesignerBackspace,
                (Some(_), Char(c)) => Command::DesignerChar(c),
                (None, Up | Char('k')) => Command::DesignerMove(-1),
                (None, Down | Char('j')) => Command::DesignerMove(1),
                (None, Char(' ')) => Command::DesignerToggle,
                (None, Enter) => Command::DesignerEditBegin,
                (None, F(2)) => Command::DesignerRun,
                (None, F(6)) => Command::DesignerSave,
                (None, F(1)) => Command::Help,
                (None, Esc) => Command::Back,
                _ => return None,
            });
        }
        if let Overlay::Create(st) = &self.overlay {
            return Some(match (&st.editing, key.code) {
                (Some(_), Enter) => Command::DesignerCommit,
                (Some(_), Esc) => Command::Back,
                (Some(_), Backspace) => Command::DesignerBackspace,
                (Some(_), Char(c)) => Command::DesignerChar(c),
                (None, Up) => Command::DesignerMove(-1),
                (None, Down | Tab) => Command::DesignerMove(1),
                (None, BackTab) => Command::DesignerMove(-1),
                (None, F(3)) => Command::DesignerCycle,
                (None, F(4)) => Command::CreatePk,
                (None, F(5)) => Command::CreateNull,
                (None, F(6)) => Command::CreateUnique,
                (None, F(7)) => Command::DesignerEditAlt,
                (None, F(10)) => Command::CreateRefs,
                (None, F(8) | Insert) => Command::DesignerAdd,
                (None, F(9) | Delete) => Command::DesignerDelete,
                (None, Char('[')) => Command::DesignerSwap(-1),
                (None, Char(']')) => Command::DesignerSwap(1),
                (None, Enter) => Command::DesignerEditBegin,
                (None, F(2)) => Command::DesignerRun,
                (None, F(1)) => Command::Help,
                (None, Esc) => Command::Back,
                // Plain letters TYPE the field (or table) name.
                (None, Char(c))
                    if !key
                        .modifiers
                        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
                {
                    Command::CreateType(c)
                }
                _ => return None,
            });
        }
        if matches!(self.overlay, Overlay::Pager(_)) {
            return Some(match key.code {
                Esc | Char('q') => Command::Back,
                Up | Char('k') => Command::PagerScroll(-1),
                Down | Char('j') => Command::PagerScroll(1),
                PageUp => Command::PagerScroll(-40),
                PageDown | Char(' ') => Command::PagerScroll(40),
                Home | Char('g') => Command::PagerScroll(i64::MIN / 2),
                End | Char('G') => Command::PagerScroll(i64::MAX / 2),
                Char('w') => Command::PagerWrite,
                Char('p') => Command::PagerPrint,
                F(1) => Command::Help,
                _ => return None,
            });
        }
        if let Overlay::Form(st) = &self.overlay {
            return Some(match (&st.editing, key.code) {
                (Some(_), Enter) => Command::DesignerCommit,
                (Some(_), Esc) => Command::Back,
                (Some(_), Backspace) => Command::DesignerBackspace,
                (Some(_), Char(c)) => Command::DesignerChar(c),
                (None, Up | Char('k')) => Command::DesignerMove(-1),
                (None, Down | Char('j')) => Command::DesignerMove(1),
                (None, Char(' ')) => Command::DesignerToggle,
                (None, Char('r')) => Command::DesignerCycle,
                (None, Char('m')) => Command::DesignerEditMask,
                (None, Char('c')) => Command::DesignerEditComputed,
                (None, Char('[')) => Command::DesignerSwap(-1),
                (None, Char(']')) => Command::DesignerSwap(1),
                (None, Enter) => Command::DesignerEditBegin,
                (None, F(2)) => Command::DesignerRun, // → the painter
                (None, F(6)) => Command::DesignerSave,
                (None, F(1)) => Command::Help,
                (None, Esc) => Command::Back,
                _ => return None,
            });
        }
        if let Overlay::Paint(st) = &self.overlay {
            return Some(match (&st.editing, key.code) {
                (Some(_), Enter) => Command::DesignerCommit,
                (Some(_), Esc) => Command::Back,
                (Some(_), Backspace) => Command::DesignerBackspace,
                (Some(_), Char(c)) => Command::DesignerChar(c),
                (None, Up | Char('k')) => Command::PaintMove { dx: 0, dy: -1 },
                (None, Down | Char('j')) => Command::PaintMove { dx: 0, dy: 1 },
                (None, Left | Char('h')) => Command::PaintMove { dx: -1, dy: 0 },
                (None, Right | Char('l')) => Command::PaintMove { dx: 1, dy: 0 },
                (None, Tab) => Command::DesignerCycle,
                (None, Char(' ')) => Command::PaintPlace,
                (None, Char('b')) => Command::PaintBox,
                (None, Char('t')) => Command::PaintText,
                (None, Char('x')) => Command::PaintDelete,
                (None, Char('+') | Char('=')) => Command::DesignerSwap(1),
                (None, Char('-')) => Command::DesignerSwap(-1),
                (None, F(6)) => Command::DesignerSave,
                (None, F(1)) => Command::Help,
                (None, Esc) => Command::Back,
                _ => return None,
            });
        }
        if let Overlay::Apps(st) = &self.overlay {
            return Some(match (&st.editing, key.code) {
                (Some(_), Enter) => Command::DesignerCommit,
                (Some(_), Esc) => Command::Back,
                (Some(_), Backspace) => Command::DesignerBackspace,
                (Some(_), Char(c)) => Command::DesignerChar(c),
                (None, Up | Char('k')) => Command::DesignerMove(-1),
                (None, Down | Char('j')) => Command::DesignerMove(1),
                (None, Char('n')) => Command::DesignerAdd,
                (None, Char('x')) => Command::DesignerDelete,
                (None, Char('c')) => Command::DesignerCycle,
                (None, Enter) => Command::DesignerEditBegin,
                (None, Char('e') | Tab) => Command::DesignerEditAlt,
                (None, Char('r')) => Command::RenameApp,
                (None, Char('E')) => Command::OpenSelectedScript,
                (None, Char('[')) => Command::DesignerSwap(-1),
                (None, Char(']')) => Command::DesignerSwap(1),
                (None, F(2)) => Command::DesignerRun,
                (None, F(1)) => Command::Help,
                (None, Esc) => Command::Back,
                _ => return None,
            });
        }
        if matches!(self.overlay, Overlay::AppMenu(_)) {
            return Some(match key.code {
                Esc | Char('0') => Command::Back,
                Up => Command::DesignerMove(-1),
                Down => Command::DesignerMove(1),
                Enter => Command::DesignerRun,
                F(1) => Command::Help,
                Char(c) => Command::DesignerChar(c), // hotkey jump-and-run
                _ => return None,
            });
        }
        if key.code == F(1) {
            return Some(Command::Help);
        }
        if key.code == F(10) {
            return Some(Command::OpenHealth);
        }
        match self.focus {
            Focus::Prompt => {
                let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
                Some(match key.code {
                    Enter => Command::PromptRun,
                    Esc => Command::Back,
                    Backspace => Command::PromptBackspace,
                    Left => Command::PromptMove(-1),
                    Right => Command::PromptMove(1),
                    Up => Command::PromptHistory(-1),
                    Down => Command::PromptHistory(1),
                    Home => Command::PromptMove(i64::MIN / 2),
                    End => Command::PromptMove(i64::MAX / 2),
                    Char('a') if ctrl => Command::PromptMove(i64::MIN / 2),
                    Char('e') if ctrl => Command::PromptMove(i64::MAX / 2),
                    Char('u') if ctrl => Command::PromptClear,
                    Char('w') if ctrl => Command::PromptDeleteWord,
                    Tab => Command::PromptComplete,
                    Char(c) => Command::PromptChar(c),
                    _ => return None,
                })
            }
            Focus::Sidebar => Some(match key.code {
                Char('q') => Command::Quit,
                Up | Char('k') => Command::SidebarMove(-1),
                Down | Char('j') => Command::SidebarMove(1),
                Enter => Command::OpenSelected,
                Char('E') => Command::OpenTableEditor,
                Char('Q') => Command::OpenQbe(None),
                Char('R') => Command::OpenReport(None),
                Char('L') => Command::OpenLabels(None),
                Char('F') => Command::OpenForm(None),
                Char('A') => Command::OpenApps(None),
                Char('C') => Command::OpenCreate(None),
                Char('.') => Command::Focus(Focus::Prompt),
                Tab => {
                    if self.grid.is_some() {
                        Command::Focus(Focus::Grid)
                    } else {
                        Command::Focus(Focus::Prompt)
                    }
                }
                Char('r') => Command::Refresh,
                Char('i') => Command::ToggleInternals,
                // First-letter seek (cycling). j/k/q/r/i stay mnemonic;
                // seek covers everything else, including '_' and digits.
                Char(c) if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' => {
                    Command::SidebarSeek(c)
                }
                _ => return None,
            }),
            Focus::Grid => Some(match key.code {
                Esc => Command::Back,
                Char('v') => Command::ToggleSplit,
                Char('H') => Command::ToggleSplitDir,
                Char('E') => Command::OpenTableEditor,
                Up | Char('k') => Command::GridMove { dr: -1, dc: 0 },
                Down | Char('j') => Command::GridMove { dr: 1, dc: 0 },
                Left | Char('h') => Command::GridMove { dr: 0, dc: -1 },
                Right | Char('l') => Command::GridMove { dr: 0, dc: 1 },
                PageUp => Command::GridPage(-1),
                PageDown => Command::GridPage(1),
                Home => Command::GridEdge(false),
                End => Command::GridEdge(true),
                Char('g') => Command::GridTop,
                Char('G') => Command::GridBottom,
                Enter => Command::OpenEdit,
                Char('a') | Insert => Command::OpenInsert,
                Char('x') | Delete => Command::DeleteRow,
                Char('n') => Command::FindNext,
                F(5) => Command::Refresh,
                Char('Q') => Command::OpenQbe(None),
                Char('R') => Command::OpenReport(None),
                Char('L') => Command::OpenLabels(None),
                Char('F') => Command::OpenForm(None),
                Char('A') => Command::OpenApps(None),
                Char('.') => Command::Focus(Focus::Prompt),
                // Tab bounces between the linked panes when split is
                // open; otherwise it heads for the dot prompt.
                Tab => {
                    if self.detail.is_some() {
                        Command::Focus(Focus::Detail)
                    } else {
                        Command::Focus(Focus::Prompt)
                    }
                }
                _ => return None,
            }),
            Focus::Detail => Some(match key.code {
                Esc => Command::Back,
                Char('v') => Command::ToggleSplit,
                Char('H') => Command::ToggleSplitDir,
                Up | Char('k') => Command::GridMove { dr: -1, dc: 0 },
                Down | Char('j') => Command::GridMove { dr: 1, dc: 0 },
                Left | Char('h') => Command::GridMove { dr: 0, dc: -1 },
                Right | Char('l') => Command::GridMove { dr: 0, dc: 1 },
                PageUp => Command::GridPage(-1),
                PageDown => Command::GridPage(1),
                Home => Command::GridEdge(false),
                End => Command::GridEdge(true),
                Char('g') => Command::GridTop,
                Char('G') => Command::GridBottom,
                Enter => Command::OpenEdit,
                Tab => Command::Focus(Focus::Grid),
                Char('.') => Command::Focus(Focus::Prompt),
                _ => return None,
            }),
        }
    }

    // ── the bus ──────────────────────────────────────────────────────

    /// Commands that mutate the database (or open an editor whose only
    /// purpose is to). Used by the `--readonly` kiosk guard.
    fn is_write_command(cmd: &Command) -> bool {
        matches!(
            cmd,
            Command::OpenInsert
                | Command::EditSave
                | Command::EditCommitField
                | Command::EditPage(_)
                | Command::DeleteRow
                | Command::DesignerSave
                | Command::DesignerAdd
                | Command::DesignerDelete
                | Command::DesignerSwap(_)
                | Command::DesignerCommit
        )
    }

    pub fn apply(&mut self, cmd: Command) {
        // The command bus is the ONLY place state changes, so one flag
        // here drives the main loop's dirty-flag redraw (main.rs).
        self.dirty = true;
        // Read-only kiosk (`--app --readonly`): every write-shaped
        // command is refused centrally, before any mutation.
        if self.readonly && Self::is_write_command(&cmd) {
            self.err("read-only mode: this application does not allow edits");
            return;
        }
        let is_delete = matches!(cmd, Command::DeleteRow);
        match cmd {
            Command::Quit => self.quit = true,
            Command::EditType(c) => {
                if matches!(&self.overlay, Overlay::Edit(ed) if ed.editing.is_none()) {
                    self.apply(Command::EditBegin); // fresh: 1st char replaces
                }
                self.apply(Command::EditChar(c));
            }
            Command::CreateRefs => {
                self.editor_fresh = true;
                if let Overlay::Create(st) = &mut self.overlay {
                    if let Some(f) = st.field_idx().and_then(|i| st.draft.fields.get(i)) {
                        st.slot = crate::creator::EditSlot::Refs;
                        st.editing = Some(f.references.clone());
                    }
                }
            }
            Command::EditOpenLink(i) => {
                let link = match &self.overlay {
                    Overlay::Edit(ed) => ed.links.get(i).map(|l| {
                        let qc = l.child.replace('"', "\"\"");
                        let qk = l.child_col.replace('"', "\"\"");
                        format!("SELECT * FROM \"{qc}\" WHERE \"{qk}\" = {}", l.key_sql)
                    }),
                    _ => None,
                };
                if let Some(sql) = link {
                    self.overlay = Overlay::None;
                    self.run_select(&sql);
                }
            }
            Command::EditPick => self.open_picker(),
            Command::PickerMove(d) => {
                if let Overlay::Edit(ed) = &mut self.overlay {
                    if let Some(p) = &mut ed.picker {
                        let n = p.rows.len();
                        if n > 0 {
                            p.cursor = (p.cursor as i64 + d).rem_euclid(n as i64) as usize;
                        }
                    }
                }
            }
            Command::PickerCancel => {
                if let Overlay::Edit(ed) = &mut self.overlay {
                    ed.picker = None;
                }
            }
            Command::PickerCommit => {
                let picked = match &self.overlay {
                    Overlay::Edit(ed) => ed.picker.as_ref().and_then(|p| {
                        p.rows
                            .get(p.cursor)
                            .and_then(|r| r.get(p.key_col))
                            .map(|v| {
                                (
                                    p.field,
                                    match v {
                                        PValue::Null => String::new(),
                                        v => v.render(),
                                    },
                                )
                            })
                    }),
                    _ => None,
                };
                if let (Some((field, value)), Overlay::Edit(ed)) = (picked, &mut self.overlay) {
                    if field < ed.inputs.len() {
                        ed.inputs[field] = Some(value);
                    }
                    ed.picker = None;
                }
            }
            Command::CreateType(c) => {
                if matches!(&self.overlay, Overlay::Create(st) if st.editing.is_none()) {
                    self.apply(Command::DesignerEditBegin); // fresh likewise
                }
                self.apply(Command::DesignerChar(c));
            }
            Command::Focus(f) => self.focus = f,
            Command::Back => self.back(),
            Command::Help => self.open_help(),
            Command::HelpScroll(d) => {
                if let Overlay::Help(st) = &mut self.overlay {
                    st.scroll = (st.scroll as i64 + d).clamp(0, 500) as u16;
                }
            }
            Command::HelpTopic(d) => {
                if let Overlay::Help(st) = &mut self.overlay {
                    let n = help::TOPICS.len() as i64;
                    st.topic = (st.topic as i64 + d).rem_euclid(n) as usize;
                    st.scroll = 0;
                }
            }
            Command::Refresh => self.refresh(),
            Command::SidebarMove(d) => {
                let n = self.visible_tables().len() as i64;
                if n > 0 {
                    self.sidebar_idx = (self.sidebar_idx as i64 + d).rem_euclid(n) as usize;
                }
            }
            Command::OpenSelected => self.open_selected(),
            Command::ToggleSplit => self.toggle_split(),
            Command::ToggleSplitDir => {
                self.split_horizontal = !self.split_horizontal;
                store::pref_set(
                    self.db.link(),
                    "split:dir",
                    if self.split_horizontal { "h" } else { "v" },
                );
                self.say(if self.split_horizontal {
                    "split: stacked (master above detail)"
                } else {
                    "split: side by side"
                });
            }
            Command::OpenTableEditor => self.open_table_editor(),
            Command::SidebarClick(idx) => {
                let n = self.visible_tables().len();
                if idx < n {
                    if self.sidebar_idx == idx && self.focus == Focus::Sidebar {
                        self.open_selected(); // same row clicked again: open
                    } else {
                        self.sidebar_idx = idx;
                        self.focus = Focus::Sidebar;
                    }
                }
            }
            Command::GridClick { row } => {
                if self.focus == Focus::Detail {
                    self.detail_jump(row);
                } else {
                    let target = self.click_target();
                    let already = self
                        .grid
                        .as_ref()
                        .is_some_and(|g| g.cur_row == row && target == Some("grid"));
                    self.focus = Focus::Grid;
                    self.grid_jump(row);
                    if already {
                        self.open_edit(); // same master row clicked again
                    }
                }
            }
            Command::GridScroll(d) => {
                if self.focus == Focus::Detail {
                    self.detail_move(d, 0);
                } else {
                    self.grid_move(d, 0);
                }
            }
            // Grid commands land on whichever pane has focus; master
            // movement re-links the detail pane inside grid_move itself.
            Command::GridMove { dr, dc } => {
                if self.focus == Focus::Detail {
                    self.detail_move(dr, dc);
                } else {
                    self.grid_move(dr, dc);
                }
            }
            Command::GridPage(dir) => {
                let step = dir * self.visible_rows.max(1);
                if self.focus == Focus::Detail {
                    self.detail_move(step, 0);
                } else {
                    self.grid_move(step, 0);
                }
            }
            Command::GridEdge(end) => {
                if self.focus == Focus::Detail {
                    if let Some(state) = &mut self.detail {
                        let g = &mut state.grid;
                        g.cur_col = if end {
                            g.columns.len().saturating_sub(1)
                        } else {
                            0
                        };
                    }
                    self.detail_move(0, 0);
                } else {
                    if let Some(g) = &mut self.grid {
                        g.cur_col = if end {
                            g.columns.len().saturating_sub(1)
                        } else {
                            0
                        };
                    }
                    self.grid_move(0, 0);
                }
            }
            Command::GridTop => {
                if self.focus == Focus::Detail {
                    self.detail_jump(0);
                } else {
                    self.grid_jump(0);
                }
            }
            Command::GridBottom => {
                if self.focus == Focus::Detail {
                    let total = self.detail.as_ref().map_or(0, |d| d.grid.total);
                    self.detail_jump(total.saturating_sub(1));
                } else {
                    let total = self.grid.as_ref().map_or(0, |g| g.total);
                    self.grid_jump(total.saturating_sub(1));
                }
            }
            Command::OpenEdit => self.open_edit(),
            Command::EditMove(d) => {
                self.fold_editing_buffer();
                if let Overlay::Edit(ed) = &mut self.overlay {
                    let n = ed.fields.len();
                    if n > 0 && d != 0 {
                        // Move `d` fields, skipping computed (read-only)
                        // columns: you can't type into a calculated one.
                        let step = d.signum();
                        let mut cur = ed.cursor;
                        for _ in 0..d.unsigned_abs() {
                            for _ in 0..n {
                                cur = (cur as i64 + step).rem_euclid(n as i64) as usize;
                                if !matches!(ed.computed.get(cur), Some(Some(_))) {
                                    break;
                                }
                            }
                        }
                        ed.cursor = cur;
                    }
                }
            }
            Command::EditBegin => {
                if let Overlay::Edit(ed) = &mut self.overlay {
                    if matches!(ed.computed.get(ed.cursor), Some(Some(_))) {
                        self.say("computed field — read-only");
                        return;
                    }
                    let current = ed.inputs[ed.cursor].clone().unwrap_or_else(|| {
                        match &ed.fields[ed.cursor].1 {
                            PValue::Null => String::new(),
                            v => v.render(),
                        }
                    });
                    let mask = ed.masks.get(ed.cursor).cloned().unwrap_or_default();
                    ed.editing = Some(apply_mask(&mask, &current));
                    self.editor_fresh = true;
                }
            }
            Command::EditChar(c) => {
                let fresh = std::mem::take(&mut self.editor_fresh);
                if let Overlay::Edit(ed) = &mut self.overlay {
                    let mask = ed.masks.get(ed.cursor).cloned().unwrap_or_default();
                    if let Some(buf) = &mut ed.editing {
                        if fresh {
                            buf.clear(); // first keystroke replaces prefill
                        }
                        buf.push(c);
                        // A PICTURE mask reflows as you type (literals
                        // inserted, non-fitting characters dropped).
                        if !mask.is_empty() {
                            *buf = apply_mask(&mask, buf);
                        }
                    }
                }
            }
            Command::EditBackspace => {
                self.editor_fresh = false; // Backspace = edit the prefill
                if let Overlay::Edit(ed) = &mut self.overlay {
                    if let Some(buf) = &mut ed.editing {
                        buf.pop();
                    }
                }
            }
            Command::EditCommitField => {
                // Enter commits the field, advances to the next one, and
                // SAVES the record when it validates (user preference:
                // Enter is the save key; F10/Ctrl-S remain save-and-
                // close). During a NEW record with required fields still
                // empty, the save quietly waits for them — no error toast
                // until an explicit save/leave is attempted.
                self.fold_editing_buffer();
                // A field was committed: the OnChange moment (rule 4).
                let _ = self.run_edit_script("OnChange", false, false);
                // An untouched NEW form must not INSERT on Enter: the
                // first Enter just advances (issue #11). F10/Ctrl-S still
                // inserts a defaults-only row if the user really wants it.
                let can_autosave = match &self.overlay {
                    Overlay::Edit(ed) => ed.dirty() || !ed.inserting,
                    _ => true,
                };
                if let Overlay::Edit(ed) = &mut self.overlay {
                    let n = ed.fields.len();
                    if n > 0 {
                        ed.cursor = (ed.cursor + 1) % n;
                    }
                }
                if self.edit_required_ok() && can_autosave {
                    // Already validated quietly; skip the loud re-check
                    // inside commit_edit to avoid a second PValue::parse
                    // per required field.
                    self.commit_edit_inner(true);
                }
            }
            Command::EditSave => self.edit_save(),
            Command::PromptChar(c) => {
                // The dot prompt IS the dot: a habitual '.' typed into
                // an empty prompt is a no-op, not a syntax error later.
                if c == '.' && self.prompt.input.is_empty() {
                    return;
                }
                // Cursor is a BYTE index, always kept on a char boundary:
                // insert/move/delete are O(1)-ish, no nth()/count() scans.
                let cur = self.prompt.cursor.min(self.prompt.input.len());
                self.prompt.input.insert(cur, c);
                self.prompt.cursor = cur + c.len_utf8();
            }
            Command::PromptBackspace => {
                let cur = self.prompt.cursor.min(self.prompt.input.len());
                if cur > 0 {
                    let prev_len = self.prompt.input[..cur]
                        .chars()
                        .next_back()
                        .map_or(1, |c| c.len_utf8());
                    let from = cur - prev_len;
                    self.prompt.input.drain(from..cur);
                    self.prompt.cursor = from;
                }
            }
            Command::PromptMove(d) => {
                let len = self.prompt.input.len();
                let mut byte = self.prompt.cursor.min(len);
                if d > 0 {
                    for _ in 0..d {
                        if byte >= len {
                            break;
                        }
                        byte += self.prompt.input[byte..]
                            .chars()
                            .next()
                            .map_or(1, |c| c.len_utf8());
                    }
                } else {
                    for _ in 0..-d {
                        if byte == 0 {
                            break;
                        }
                        byte -= self.prompt.input[..byte]
                            .chars()
                            .next_back()
                            .map_or(1, |c| c.len_utf8());
                    }
                }
                self.prompt.cursor = byte;
            }
            Command::PromptHistory(d) => self.prompt_history(d),
            Command::PromptRun => self.prompt_run(),
            Command::OpenHealth => self.open_health(),
            Command::HealthSample => self.health_sample(),
            Command::OpenQbe(t) => self.open_qbe(t),
            Command::OpenReport(t) => self.open_report(t),
            Command::OpenLabels(t) => self.open_labels(t),
            Command::DesignerMove(d) => self.designer_move(d),
            Command::DesignerToggle => self.designer_toggle(),
            Command::DesignerCycle => self.designer_cycle(),
            Command::DesignerJoin => self.designer_join(),
            Command::DesignerGroup => self.designer_group(),
            Command::DesignerEditBegin => self.designer_edit_begin(),
            Command::DesignerChar(c) => self.designer_char(c),
            Command::DesignerBackspace => self.designer_backspace(),
            Command::DesignerCommit => self.designer_commit(),
            Command::DesignerRun => self.designer_run(),
            Command::DesignerSave => self.designer_save(),
            Command::PagerScroll(d) => {
                if let Overlay::Pager(p) = &mut self.overlay {
                    let max = p.lines.len().saturating_sub(10) as i64;
                    p.offset = (p.offset as i64).saturating_add(d).clamp(0, max) as usize;
                }
            }
            Command::PagerWrite => {
                if let Overlay::Pager(p) = &self.overlay {
                    match p.write_file() {
                        Ok(path) => self.say(format!("wrote {path}")),
                        Err(e) => self.err(e),
                    }
                }
            }
            Command::PagerPrint => {
                if let Overlay::Pager(p) = &self.overlay {
                    match p.print_via_lp() {
                        Ok(msg) => self.say(msg),
                        Err(e) => self.err(e),
                    }
                }
            }
            Command::DesignerAdd => self.designer_add(),
            Command::DesignerDelete => self.designer_delete(),
            Command::DesignerSwap(d) => self.designer_swap(d),
            Command::DesignerEditAlt => self.designer_edit_alt(),
            Command::DesignerEditMask => self.designer_edit_mask(),
            Command::DesignerEditComputed => self.designer_edit_computed(),
            Command::OpenForm(t) => self.open_form(t),
            Command::OpenTable(name) => self.open_table(&name),
            Command::OpenSavedQuery(name) => match QbeSpec::saved_sql(self.db.link(), &name) {
                Some(sql) => self.run_select(&sql),
                None => self.err(format!("no saved query named {name:?}")),
            },
            Command::OpenSavedReport(name) => {
                let spec = ReportSpec::load(self.db.link(), &name)
                    .unwrap_or_else(|| ReportSpec::for_table(&name));
                match report::render(self.db.link(), &spec) {
                    Ok(lines) => {
                        self.overlay = Overlay::Pager(PagerState {
                            title: format!("REPORT · {}", spec.title),
                            lines,
                            offset: 0,
                            file_stem: format!("report_{}", spec.name),
                        })
                    }
                    Err(e) => self.err(e),
                }
            }
            Command::OpenFormScript { table, event } => self.open_form_script(table, event),
            Command::OpenSelectedScript => self.open_selected_item_script(),
            Command::RenameApp => self.rename_app_begin(),
            Command::ScriptChar(c) => self.script_char(c),
            Command::ScriptTab => {
                self.script_char(' ');
                self.script_char(' ');
            }
            Command::ScriptNewline => self.script_newline(),
            Command::ScriptBackspace => self.script_backspace(),
            Command::ScriptMove { dl, dc } => self.script_move(dl, dc),
            Command::ScriptLineEdge(end) => self.script_line_edge(end),
            Command::ScriptSave => self.script_save(),
            Command::OpenApps(name) => self.open_apps(name),
            Command::OpenAppMenu(name) => self.open_app_menu(name),
            Command::OpenInsert => self.open_insert(),
            Command::EditPage(d) => self.edit_page(d),
            Command::DeleteRow => self.delete_row(),
            Command::FindNext => self.find_next(),
            Command::DbReady(tag, took, resp) => self.finish_db(tag, took, resp),
            Command::PromptClear => {
                self.prompt.input.clear();
                self.prompt.cursor = 0;
            }
            Command::PromptDeleteWord => {
                // Byte-wise backward word erase over char boundaries.
                let len = self.prompt.input.len();
                let mut i = self.prompt.cursor.min(len);
                let at = |s: &str, j: usize| s[..j].chars().next_back().unwrap_or(' ');
                while i > 0 && at(&self.prompt.input, i).is_whitespace() {
                    i -= self.prompt.input[..i]
                        .chars()
                        .next_back()
                        .map_or(1, |c| c.len_utf8());
                }
                while i > 0 && !at(&self.prompt.input, i).is_whitespace() {
                    i -= self.prompt.input[..i]
                        .chars()
                        .next_back()
                        .map_or(1, |c| c.len_utf8());
                }
                let cur = self.prompt.cursor.min(len);
                self.prompt.input.drain(i..cur);
                self.prompt.cursor = i;
            }
            Command::PromptComplete => self.prompt_complete(),
            Command::OpenCreate(name) => self.open_create(name),
            Command::CreatePk => self.create_toggle(|f| f.pk = !f.pk),
            Command::CreateNull => self.create_toggle(|f| f.notnull = !f.notnull),
            Command::CreateUnique => self.create_toggle(|f| f.unique = !f.unique),
            Command::SidebarSeek(c) => self.sidebar_seek(c),
            Command::ToggleInternals => {
                self.show_internals = !self.show_internals;
                self.sidebar_idx = 0;
                self.say(if self.show_internals {
                    "internal tables shown (i to hide)"
                } else {
                    "internal tables hidden (i to show)"
                });
            }
            Command::PaintMove { dx, dy } => self.paint_move(dx, dy),
            Command::PaintPlace => self.paint_place(),
            Command::PaintBox => self.paint_box(),
            Command::PaintText => {
                if let Overlay::Paint(st) = &mut self.overlay {
                    st.editing = Some(String::new());
                }
            }
            Command::PaintDelete => self.paint_delete(),
        }
        // Any command other than a second DeleteRow disarms the pending
        // delete (moving the cursor, refreshing, anything).
        if !is_delete {
            self.pending_delete = None;
        }
    }

    fn back(&mut self) {
        // A parked EDIT target belongs to a form that's about to close:
        // otherwise a still-flying window would resurrect it (the user
        // Esc'd once; the form must stay closed).
        if matches!(self.overlay, Overlay::Edit(_)) {
            self.pending_edit = None;
        }
        match &mut self.overlay {
            Overlay::Edit(ed) if ed.editing.is_some() => ed.editing = None,
            Overlay::Qbe(st) if st.editing.is_some() => {
                st.editing = None;
                st.naming = false;
            }
            Overlay::Report(st) if st.editing.is_some() => {
                st.editing = None;
                st.naming = false;
            }
            Overlay::Form(st) if st.editing.is_some() => st.editing = None,
            Overlay::Apps(st) if st.editing.is_some() => {
                st.editing = None;
                st.renaming_app = false;
            }
            Overlay::Create(st) if st.editing.is_some() => {
                st.editing = None;
                st.slot = crate::creator::EditSlot::Name;
            }
            Overlay::Paint(st) if st.editing.is_some() => st.editing = None,
            Overlay::Paint(st) if st.pending_box.is_some() => st.pending_box = None,
            Overlay::Paint(_) => {
                // Painter backs out to the list designer, same spec.
                if let Overlay::Paint(st) = std::mem::replace(&mut self.overlay, Overlay::None) {
                    self.overlay = Overlay::Form(FormState {
                        spec: st.spec,
                        cursor: 0,
                        editing: None,
                        editing_mask: false,
                        editing_computed: false,
                    });
                }
            }
            Overlay::Pager(_) if self.menu_launched => {
                // Home means home: a menu-launched pager closes back to
                // the application menu, not to the bare browser.
                self.menu_launched = false;
                let home = self.app_home.clone();
                self.open_app_menu(home);
            }
            Overlay::ScriptEditor(_) => self.close_script_editor(),
            Overlay::Edit(_)
            | Overlay::Help(_)
            | Overlay::Health(_)
            | Overlay::Qbe(_)
            | Overlay::Report(_)
            | Overlay::Pager(_)
            | Overlay::Form(_)
            | Overlay::Apps(_)
            | Overlay::Create(_)
            | Overlay::AppMenu(_) => self.overlay = Overlay::None,
            Overlay::None => match self.focus {
                Focus::Prompt => {
                    self.focus = if self.grid.is_some() {
                        Focus::Grid
                    } else {
                        Focus::Sidebar
                    }
                }
                Focus::Detail => self.close_detail(), // Esc: pane first…
                Focus::Grid => self.focus = Focus::Sidebar, // …then sidebar
                Focus::Sidebar => match self.app_home.clone() {
                    // App mode: the top level IS the application menu.
                    Some(home) => self.open_app_menu(Some(home)),
                    None => self.say("q quits (Esc has nothing to back out of)"),
                },
            },
        }
    }

    fn refresh(&mut self) {
        self.reload_tables();
        // Async refill of the live window (columns/widths stay put);
        // the grid re-seeks when the window arrives.
        let target = match &self.grid {
            Some(g) => match &g.source {
                GridSource::Table { name, .. } => Some((
                    name.clone(),
                    g.cur_row,
                    g.cur_col,
                    g.cache_start,
                    (g.cache.len() as i64).max(1),
                )),
                _ => None,
            },
            None => None,
        };
        if let Some((name, row, col, want_start, limit)) = target {
            let job_name = name.clone();
            let submitted = self.db.submit(Box::new(move |db| {
                DbResponse::Window(db.open_window(&job_name, want_start, limit))
            }));
            match submitted {
                Some(tag) => {
                    self.pending.insert(
                        tag,
                        PendingOp::Refill {
                            name,
                            row,
                            col,
                            want_start,
                        },
                    );
                }
                None => self.err("database worker is gone"),
            }
        }
        // Explicit user refresh: force a fresh dot, not the cache.
        self.invalidate_health();
        self.refresh_health();
        self.say("refreshed");
    }

    // ── phase 3: the dbhealth console ────────────────────────────────

    fn quote_ident(ident: &str) -> String {
        format!("\"{}\"", ident.replace('"', "\"\""))
    }

    /// Series names for the health fallback path (no window functions).
    fn series_names(db: &dyn DbLink, base: &str) -> Vec<String> {
        db.query(&format!(
            "SELECT DISTINCT name FROM {} ORDER BY name",
            Self::quote_ident(base)
        ))
        .map(|q| {
            q.rows
                .into_iter()
                .filter_map(|r| match r.into_iter().next() {
                    Some(PValue::Text(t)) => Some(t),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
    }

    /// Time-based behavior between keystrokes (main loop ticks ~4x/s).
    /// While the DBHEALTH console is open it is LIVE: phosphor takes a
    /// sample every 5 seconds so the trends move on their own — the
    /// passive vtab never samples itself (that is the design), so the
    /// open console volunteers as the collector.
    pub fn tick(&mut self) {
        if matches!(self.overlay, Overlay::Health(_))
            && self.last_auto_sample.elapsed() >= std::time::Duration::from_secs(5)
        {
            self.last_auto_sample = std::time::Instant::now();
            self.health_sample();
            self.dirty = true;
        }
    }

    /// F1: open help on the topic for wherever the user is right now.
    fn open_help(&mut self) {
        let key = match &self.overlay {
            Overlay::Edit(_) => "browse",
            Overlay::Health(_) => "health",
            Overlay::Qbe(_) => "qbe",
            Overlay::Report(_) | Overlay::Pager(_) => "reports",
            Overlay::Form(_) | Overlay::Paint(_) => "forms",
            Overlay::Apps(_) | Overlay::AppMenu(_) => "apps",
            Overlay::Create(_) => "browse",
            Overlay::ScriptEditor(_) => "script",
            Overlay::Help(_) => return,
            Overlay::None => match self.focus {
                Focus::Prompt => "prompt",
                _ => "browse",
            },
        };
        self.overlay = Overlay::Help(HelpState {
            topic: help::topic_index(key),
            scroll: 0,
        });
    }

    /// The whole health console in one worker job: base discovery,
    /// report, sparklines, and dot. Pure function of the link, so the
    /// UI thread never blocks on its (up to 4) round-trips.
    fn fetch_health_console(db: &dyn DbLink) -> DbResult<crate::worker::HealthData> {
        let view: String = db
            .query(
                "SELECT name FROM sqlite_master \
                 WHERE type = 'view' AND name LIKE '%\\_report' ESCAPE '\\' \
                 ORDER BY name LIMIT 1",
            )
            .ok()
            .and_then(|q| q.rows.into_iter().next())
            .and_then(|r| r.into_iter().next())
            .and_then(|v| match v {
                PValue::Text(t) => Some(t),
                _ => None,
            })
            .filter(|v| v.ends_with("_report"))
            .ok_or_else(|| {
                "no dbhealth here — needs the timeless extension and \
                 CREATE VIRTUAL TABLE dbhealth USING timeless_health"
                    .to_owned()
            })?;
        let base = view.strip_suffix("_report").unwrap_or(&view).to_owned();
        let report = db
            .query(&format!(
                "SELECT \"check\", status, value, advice FROM {}",
                Self::quote_ident(&view)
            ))?
            .rows
            .into_iter()
            .map(|r| [0, 1, 2, 3].map(|i| r.get(i).map(PValue::render).unwrap_or_default()))
            .collect();

        // Sparklines: preferred series first, then whatever else exists.
        // ONE windowed query replaces DISTINCT + N per-series round-trips
        // (was up to 9 trips per open and per 5s auto-sample): newest 64
        // samples per series, grouped client-side.
        const PREFERRED: [&str; 8] = [
            "cache_hit_ratio",
            "db_file_bytes",
            "wal_file_bytes",
            "bloat_ratio",
            "cache_misses",
            "cache_hits",
            "cache_used_bytes",
            "memory_used_bytes",
        ];
        let mut by_name: BTreeMap<String, Vec<f64>> = BTreeMap::new();
        match db.query(&format!(
            "SELECT name, value FROM \
             (SELECT name, value, row_number() OVER \
              (PARTITION BY name ORDER BY ts DESC) AS rn FROM {}) \
             WHERE rn <= 64 ORDER BY name",
            Self::quote_ident(&base)
        )) {
            Ok(q) => {
                for r in q.rows {
                    let Some(PValue::Text(name)) = r.first() else {
                        continue;
                    };
                    let v = match r.get(1) {
                        Some(PValue::Real(f)) => *f,
                        Some(PValue::Int(i)) => *i as f64,
                        _ => continue,
                    };
                    by_name.entry(name.clone()).or_default().push(v);
                }
                // Rows arrived newest-first per series; sparks read oldest-first.
                for vals in by_name.values_mut() {
                    vals.reverse();
                }
            }
            Err(_) => {
                // Old SQLite / quirky vtab without window support:
                // fall back to the per-series loop (slower, same picture).
                for name in Self::series_names(db, &base) {
                    let safe = name.replace('\'', "''");
                    if let Ok(q) = db.query(&format!(
                        "SELECT value FROM {} WHERE name = '{safe}' ORDER BY ts DESC LIMIT 64",
                        Self::quote_ident(&base)
                    )) {
                        let mut vals: Vec<f64> = q
                            .rows
                            .into_iter()
                            .filter_map(|r| match r.into_iter().next() {
                                Some(PValue::Real(f)) => Some(f),
                                Some(PValue::Int(i)) => Some(i as f64),
                                _ => None,
                            })
                            .collect();
                        vals.reverse();
                        by_name.insert(name, vals);
                    }
                }
            }
        }
        let available: Vec<String> = by_name.keys().cloned().collect();
        let mut ordered: Vec<String> = PREFERRED
            .iter()
            .filter(|p| available.iter().any(|a| a == *p))
            .map(|s| s.to_string())
            .collect();
        for a in &available {
            if ordered.len() >= 8 {
                break;
            }
            if !ordered.contains(a) {
                ordered.push(a.clone());
            }
        }

        let mut sparks = Vec::new();
        for name in ordered {
            if let Some(vals) = by_name.get(&name) {
                if let Some(latest) = vals.last().copied() {
                    let rendered = if latest.abs() >= 1_048_576.0 {
                        format!("{:.1} MB", latest / 1_048_576.0)
                    } else if latest.fract() == 0.0 {
                        format!("{latest:.0}")
                    } else {
                        format!("{latest:.3}")
                    };
                    sparks.push((name, vals.clone(), rendered));
                }
            }
        }

        Ok(crate::worker::HealthData {
            base,
            report,
            sparks,
            health: db.health(),
        })
    }

    fn open_health(&mut self) {
        // Async: the bundle (base + report + sparks + dot) arrives as
        // one job; the current screen stays put until the console does.
        // The overlay discriminant guards stale installs (user moved on).
        let seq = self.status_seq;
        let overlay = std::mem::discriminant(&self.overlay);
        match self.db.submit(Box::new(move |db| {
            DbResponse::HealthConsole(Self::fetch_health_console(db))
        })) {
            Some(tag) => {
                self.pending.insert(
                    tag,
                    PendingOp::Health {
                        sampled: false,
                        seq,
                        overlay,
                    },
                );
            }
            None => self.err("database worker is gone"),
        }
    }

    fn health_sample(&mut self) {
        if self.readonly {
            return self.err("read-only mode: cannot take a health sample");
        }
        let Overlay::Health(hv) = &self.overlay else {
            return;
        };
        let t = Self::quote_ident(&hv.table);
        match self
            .db
            .execute(&format!("INSERT INTO {t}({t}) VALUES ('sample')"))
        {
            Ok((_, elapsed)) => {
                self.last_ms = Some(elapsed.as_secs_f64() * 1000.0);
                // Async rebuild (same bundle as open); "sampled" lands
                // with the fresh console so the message never lies.
                let seq = self.status_seq;
                let overlay = std::mem::discriminant(&self.overlay);
                match self.db.submit(Box::new(move |db| {
                    DbResponse::HealthConsole(Self::fetch_health_console(db))
                })) {
                    Some(tag) => {
                        self.pending.insert(
                            tag,
                            PendingOp::Health {
                                sampled: true,
                                seq,
                                overlay,
                            },
                        );
                    }
                    None => self.err("database worker is gone"),
                }
            }
            Err(e) => self.err(e),
        }
    }

    // ── phase 4: QBE, reports, labels ────────────────────────────────

    /// The table a designer should target: explicit arg, else current
    /// grid table, else the sidebar selection.
    fn target_table(&self, arg: Option<String>) -> Option<String> {
        arg.or_else(|| match &self.grid {
            Some(Grid {
                source: GridSource::Table { name, .. },
                ..
            }) if self.focus == Focus::Grid => Some(name.clone()),
            _ => self
                .visible_tables()
                .get(self.sidebar_idx)
                .map(|t| t.name.clone()),
        })
    }

    fn open_create(&mut self, name: Option<String>) {
        let name = name.unwrap_or_else(|| {
            // A default name that doesn't collide with anything.
            let mut n = 1;
            loop {
                let candidate = format!("table{n}");
                if !self.tables.iter().any(|t| t.name == candidate) {
                    break candidate;
                }
                n += 1;
            }
        });
        self.overlay = Overlay::Create(CreateState::new(&name));
    }

    /// 'E' on a table: the TABLE EDITOR — the designer preloaded with
    /// the table's live columns. F2 then applies your changes as
    /// ALTER TABLE statements (add / rename / drop; type and
    /// constraint changes to existing columns are declined).
    fn open_table_editor(&mut self) {
        let name = match &self.grid {
            Some(g) => match &g.source {
                GridSource::Table { name, .. } => name.clone(),
                _ => {
                    return self.say(
                        "the table editor works on a table (query results have no structure)",
                    );
                }
            },
            None => match self.visible_tables().get(self.sidebar_idx) {
                Some(t) => t.name.clone(),
                None => return self.say("no table selected"),
            },
        };
        let cols = match self.cached_columns(&name) {
            Ok(c) => c,
            Err(e) => return self.err(e),
        };
        let fks = self.db.outgoing_fks(&name);
        self.overlay = Overlay::Create(CreateState::edit_existing(crate::creator::EditorSchema {
            table: name,
            columns: cols,
            fks,
        }));
    }

    fn create_toggle(&mut self, f: impl FnOnce(&mut crate::creator::FieldDef)) {
        if let Overlay::Create(st) = &mut self.overlay {
            if let Some(i) = st.field_idx() {
                if let Some(field) = st.draft.fields.get_mut(i) {
                    f(field);
                }
            }
        }
    }

    fn open_qbe(&mut self, table: Option<String>) {
        let Some(table) = self.target_table(table) else {
            return self.err("qbe: no table selected (qbe <table>)");
        };
        match QbeSpec::new(self.db.link(), &table) {
            Ok(spec) => self.overlay = Overlay::Qbe(QbeState::new(spec)),
            Err(e) => self.err(e),
        }
    }

    fn open_report(&mut self, name: Option<String>) {
        let Some(name) = self.target_table(name) else {
            return self.err("report: no table selected (report <table-or-saved-name>)");
        };
        // A saved report by this name wins; otherwise start from the table.
        let (spec, original_name) = match ReportSpec::load(self.db.link(), &name) {
            Some(s) => {
                let n = s.name.clone();
                (s, Some(n))
            }
            None => (ReportSpec::for_table(&name), None),
        };
        let columns = self.source_columns(&spec);
        self.overlay = Overlay::Report(ReportState {
            spec,
            cursor: 0,
            editing: None,
            columns,
            naming: false,
            original_name,
        });
    }

    fn source_columns(&self, spec: &ReportSpec) -> Vec<String> {
        let src = spec.source.trim();
        let sql = if src.to_ascii_lowercase().starts_with("select")
            || src.to_ascii_lowercase().starts_with("with")
        {
            format!("SELECT * FROM ({src}) LIMIT 0")
        } else {
            format!("SELECT * FROM {} LIMIT 0", Self::quote_ident(src))
        };
        self.db.query(&sql).map(|q| q.columns).unwrap_or_default()
    }

    fn open_labels(&mut self, table: Option<String>) {
        let Some(table) = self.target_table(table) else {
            return self.err("labels: no table selected (labels <table>)");
        };
        match report::labels(self.db.link(), &table) {
            Ok(lines) => {
                self.overlay = Overlay::Pager(PagerState {
                    title: format!("LABELS · {table}"),
                    lines,
                    offset: 0,
                    file_stem: format!("labels_{table}"),
                })
            }
            Err(e) => self.err(e),
        }
    }

    fn designer_move(&mut self, d: i64) {
        fn wrap(cursor: &mut usize, d: i64, n: usize) {
            if n > 0 {
                *cursor = (*cursor as i64 + d).rem_euclid(n as i64) as usize;
            }
        }
        match &mut self.overlay {
            Overlay::Qbe(st) => wrap(&mut st.cursor, d, st.spec.cols.len()),
            Overlay::Report(st) => wrap(&mut st.cursor, d, 3),
            Overlay::Form(st) => wrap(&mut st.cursor, d, st.spec.fields.len()),
            Overlay::Apps(st) => wrap(&mut st.cursor, d, st.items.len()),
            Overlay::AppMenu(st) => wrap(&mut st.cursor, d, st.items.len()),
            Overlay::Create(st) => wrap(&mut st.cursor, d, st.draft.fields.len() + 1),
            _ => {}
        }
    }

    fn designer_toggle(&mut self) {
        match &mut self.overlay {
            Overlay::Qbe(st) => {
                let col = &mut st.spec.cols[st.cursor];
                col.show = !col.show;
            }
            Overlay::Form(st) => {
                if let Some(f) = st.spec.fields.get_mut(st.cursor) {
                    f.include = !f.include;
                }
            }
            Overlay::Report(st) if st.cursor == 2 => {
                // Cycle group_by through the source's columns (and off).
                let next = match &st.spec.group_by {
                    None => st.columns.first().cloned(),
                    Some(cur) => {
                        let idx = st.columns.iter().position(|c| c == cur);
                        match idx {
                            Some(i) if i + 1 < st.columns.len() => Some(st.columns[i + 1].clone()),
                            _ => None,
                        }
                    }
                };
                st.spec.group_by = next;
            }
            _ => {}
        }
    }

    /// QBE `J`: cycle through the FK-driven joins (off → each → off).
    fn designer_join(&mut self) {
        let Overlay::Qbe(st) = &mut self.overlay else {
            return;
        };
        if st.spec.relations.is_empty() {
            self.say("no declared foreign keys reach this table");
            return;
        }
        let next = match &st.spec.join {
            None => Some(st.spec.relations[0].clone()),
            Some(cur) => {
                let i = st.spec.relations.iter().position(|r| r == cur);
                match i {
                    Some(i) if i + 1 < st.spec.relations.len() => {
                        Some(st.spec.relations[i + 1].clone())
                    }
                    _ => None,
                }
            }
        };
        st.spec.join = next;
    }

    /// QBE `g`: cycle GROUP BY through the columns (off → each → off).
    fn designer_group(&mut self) {
        let Overlay::Qbe(st) = &mut self.overlay else {
            return;
        };
        let next = match &st.spec.group_by {
            None => st.spec.cols.first().map(|c| c.name.clone()),
            Some(cur) => {
                let i = st.spec.cols.iter().position(|c| &c.name == cur);
                match i {
                    Some(i) if i + 1 < st.spec.cols.len() => Some(st.spec.cols[i + 1].name.clone()),
                    _ => None,
                }
            }
        };
        st.spec.group_by = next;
    }

    fn designer_cycle(&mut self) {
        match &mut self.overlay {
            Overlay::Qbe(st) => {
                let col = &mut st.spec.cols[st.cursor];
                col.sort = col.sort.cycle();
            }
            Overlay::Form(st) => {
                if let Some(f) = st.spec.fields.get_mut(st.cursor) {
                    f.required = !f.required;
                }
            }
            Overlay::Apps(st) => {
                if let Some(item) = st.items.get_mut(st.cursor) {
                    item.kind = item.kind.cycle();
                    let item = item.clone();
                    let _ = appsgen::update_item(self.db.link(), &item);
                }
            }
            Overlay::Paint(st) => st.select_next(),
            Overlay::Create(st) => {
                if let Some(i) = st.field_idx() {
                    if let Some(f) = st.draft.fields.get_mut(i) {
                        f.ftype = f.ftype.cycle();
                    }
                }
            }
            _ => {}
        }
    }

    fn designer_edit_begin(&mut self) {
        self.editor_fresh = true;
        match &mut self.overlay {
            Overlay::Qbe(st) => {
                st.naming = false;
                st.editing = Some(st.spec.cols[st.cursor].filter.clone());
            }
            Overlay::Report(st) => {
                st.editing = Some(match st.cursor {
                    0 => st.spec.title.clone(),
                    1 => st.spec.source.clone(),
                    // Space still cycles columns; Enter types an expression.
                    _ => st.spec.group_by.clone().unwrap_or_default(),
                });
            }
            Overlay::Form(st) => {
                if let Some(f) = st.spec.fields.get(st.cursor) {
                    st.editing_mask = false;
                    st.editing_computed = false;
                    st.editing = Some(f.label.clone());
                }
            }
            Overlay::Apps(st) => {
                if let Some(item) = st.items.get(st.cursor) {
                    st.editing_ref = false;
                    st.renaming_app = false;
                    st.editing = Some(item.label.clone());
                }
            }
            Overlay::Create(st) => {
                st.slot = crate::creator::EditSlot::Name;
                st.editing = Some(match st.field_idx() {
                    None => st.draft.table.clone(),
                    Some(i) => st
                        .draft
                        .fields
                        .get(i)
                        .map(|f| f.name.clone())
                        .unwrap_or_default(),
                });
            }
            _ => {}
        }
    }

    fn designer_edit_alt(&mut self) {
        self.editor_fresh = true;
        match &mut self.overlay {
            Overlay::Apps(st) => {
                if let Some(item) = st.items.get(st.cursor) {
                    st.editing_ref = true;
                    st.renaming_app = false;
                    st.editing = Some(item.action_ref.clone());
                }
            }
            Overlay::Create(st) => {
                if let Some(i) = st.field_idx() {
                    if let Some(f) = st.draft.fields.get(i) {
                        st.slot = crate::creator::EditSlot::Default;
                        st.editing = Some(f.default.clone());
                    }
                }
            }
            _ => {}
        }
    }

    /// Forms `m`: edit the selected field's PICTURE mask.
    fn designer_edit_mask(&mut self) {
        self.editor_fresh = true;
        if let Overlay::Form(st) = &mut self.overlay {
            if let Some(f) = st.spec.fields.get(st.cursor) {
                st.editing_mask = true;
                st.editing_computed = false;
                st.editing = Some(f.mask.clone());
            }
        }
    }

    /// Forms `c`: edit the selected field's computed expression.
    fn designer_edit_computed(&mut self) {
        self.editor_fresh = true;
        if let Overlay::Form(st) = &mut self.overlay {
            if let Some(f) = st.spec.fields.get(st.cursor) {
                st.editing_computed = true;
                st.editing_mask = false;
                st.editing = Some(f.computed.clone());
            }
        }
    }

    fn designer_buffer(&mut self) -> Option<&mut String> {
        match &mut self.overlay {
            Overlay::Qbe(st) => st.editing.as_mut(),
            Overlay::Report(st) => st.editing.as_mut(),
            Overlay::Form(st) => st.editing.as_mut(),
            Overlay::Apps(st) => st.editing.as_mut(),
            Overlay::Paint(st) => st.editing.as_mut(),
            Overlay::Create(st) => st.editing.as_mut(),
            _ => None,
        }
    }

    // ── the painter (CREATE SCREEN) ──────────────────────────────────

    fn paint_move(&mut self, dx: i64, dy: i64) {
        if let Overlay::Paint(st) = &mut self.overlay {
            let (w, h) = st.spec.size;
            let nx = (st.cursor.0 as i64 + dx).clamp(0, w as i64 - 1) as u16;
            let ny = (st.cursor.1 as i64 + dy).clamp(0, h as i64 - 1) as u16;
            st.cursor = (nx, ny);
        }
    }

    /// Space: place the selected field's label at the cursor.
    fn paint_place(&mut self) {
        if let Overlay::Paint(st) = &mut self.overlay {
            if let Some(f) = st.spec.fields.get_mut(st.selected) {
                if f.include {
                    f.pos = Some(st.cursor);
                }
            }
        }
    }

    /// 'b' twice: a box from the first corner to the cursor.
    fn paint_box(&mut self) {
        if let Overlay::Paint(st) = &mut self.overlay {
            match st.pending_box.take() {
                None => st.pending_box = Some(st.cursor),
                Some((x0, y0)) => {
                    let (x1, y1) = st.cursor;
                    let (x, y) = (x0.min(x1), y0.min(y1));
                    let w = x0.abs_diff(x1) + 1;
                    let h = y0.abs_diff(y1) + 1;
                    if w >= 2 && h >= 2 {
                        st.spec.boxes.push(BoxItem { x, y, w, h });
                    } else {
                        self.say("box needs at least 2×2 (move before second b)");
                    }
                }
            }
        }
    }

    /// 'x': delete whatever sits under the cursor — a text (by its
    /// span), a box (by its corner), or unplace the field whose label
    /// starts here. Most specific first.
    fn paint_delete(&mut self) {
        if let Overlay::Paint(st) = &mut self.overlay {
            let (cx, cy) = st.cursor;
            if let Some(i) = st.spec.texts.iter().position(|t| {
                t.y == cy && cx >= t.x && (cx as usize) < t.x as usize + t.text.chars().count()
            }) {
                st.spec.texts.remove(i);
                return;
            }
            if let Some(f) = st.spec.fields.iter_mut().find(|f| f.pos == Some((cx, cy))) {
                f.pos = None;
                return;
            }
            if let Some(i) = st.spec.boxes.iter().position(|b| (b.x, b.y) == (cx, cy)) {
                st.spec.boxes.remove(i);
            }
        }
    }

    fn designer_char(&mut self, c: char) {
        // AppMenu has no buffer: letters are dBASE-style hotkeys (jump
        // to the first item whose label starts with the letter and run).
        if let Overlay::AppMenu(st) = &mut self.overlay {
            let hit = st.items.iter().position(|i| {
                i.label
                    .chars()
                    .next()
                    .is_some_and(|f| f.eq_ignore_ascii_case(&c))
            });
            if let Some(idx) = hit {
                st.cursor = idx;
                self.designer_run();
            }
            return;
        }
        let fresh = std::mem::take(&mut self.editor_fresh);
        if let Some(buf) = self.designer_buffer() {
            if fresh {
                buf.clear(); // first keystroke replaces prefill
            }
            buf.push(c);
        }
    }

    fn designer_backspace(&mut self) {
        self.editor_fresh = false; // Backspace = edit the prefill
        if let Some(buf) = self.designer_buffer() {
            buf.pop();
        }
    }

    fn designer_commit(&mut self) {
        let mut save_as: Option<String> = None;
        let mut save_report = false;
        let mut app_rename: Option<String> = None;
        match &mut self.overlay {
            Overlay::Qbe(st) => {
                if let Some(buf) = st.editing.take() {
                    if st.naming {
                        st.naming = false;
                        if !buf.trim().is_empty() {
                            save_as = Some(buf.trim().to_owned());
                        }
                    } else {
                        st.spec.cols[st.cursor].filter = buf;
                    }
                }
            }
            Overlay::Report(st) => {
                if st.naming {
                    st.naming = false;
                    if let Some(buf) = st.editing.take() {
                        let n = buf.trim().to_owned();
                        if !n.is_empty() {
                            st.spec.name = n;
                        }
                    }
                    save_report = true;
                } else if let Some(buf) = st.editing.take() {
                    match st.cursor {
                        0 => st.spec.title = buf,
                        1 => {
                            st.spec.source = buf;
                            st.spec.group_by = None;
                        }
                        // Typed grouping expression; empty clears it.
                        _ => {
                            let g = buf.trim().to_owned();
                            st.spec.group_by = (!g.is_empty()).then_some(g);
                        }
                    }
                    if st.cursor == 1 {
                        let cols = self.source_columns_of_overlay();
                        if let Overlay::Report(st) = &mut self.overlay {
                            st.columns = cols;
                        }
                        return;
                    }
                }
            }
            Overlay::Form(st) => {
                if let Some(buf) = st.editing.take() {
                    if let Some(f) = st.spec.fields.get_mut(st.cursor) {
                        if st.editing_mask {
                            f.mask = buf.trim().to_owned();
                        } else if st.editing_computed {
                            f.computed = buf.trim().to_owned();
                        } else {
                            f.label = buf;
                        }
                        st.editing_mask = false;
                        st.editing_computed = false;
                    }
                }
            }
            Overlay::Apps(st) => {
                if let Some(buf) = st.editing.take() {
                    if st.renaming_app {
                        st.renaming_app = false;
                        let new = buf.trim().to_owned();
                        if !new.is_empty() && new != st.app {
                            app_rename = Some(new);
                        }
                    } else if let Some(item) = st.items.get_mut(st.cursor) {
                        if st.editing_ref {
                            item.action_ref = buf;
                        } else {
                            item.label = buf;
                        }
                        let item = item.clone();
                        let _ = appsgen::update_item(self.db.link(), &item);
                    }
                }
            }
            Overlay::Paint(st) => {
                // Commit a static text at the cursor.
                if let Some(buf) = st.editing.take() {
                    if !buf.is_empty() {
                        let (x, y) = st.cursor;
                        st.spec.texts.push(TextItem { x, y, text: buf });
                    }
                }
            }
            Overlay::Create(st) => {
                use crate::creator::EditSlot;
                if let Some(buf) = st.editing.take() {
                    match (st.field_idx(), st.slot) {
                        (None, _) => st.draft.table = buf.trim().to_owned(),
                        (Some(i), EditSlot::Name) => {
                            if let Some(f) = st.draft.fields.get_mut(i) {
                                f.name = buf.trim().to_owned();
                            }
                        }
                        (Some(i), EditSlot::Default) => {
                            if let Some(f) = st.draft.fields.get_mut(i) {
                                f.default = buf;
                            }
                        }
                        (Some(i), EditSlot::Refs) => {
                            if let Some(f) = st.draft.fields.get_mut(i) {
                                f.references = buf.trim().to_owned();
                            }
                        }
                    }
                    st.slot = EditSlot::Name;
                }
            }
            _ => {}
        }
        if let (Some(name), Overlay::Qbe(st)) = (&save_as, &self.overlay) {
            match st.spec.save(self.db.link(), name) {
                Ok(()) => self.say(format!("saved query {name:?} (run {name})")),
                Err(e) => self.err(e),
            }
        }
        if save_report {
            let payload = match &self.overlay {
                Overlay::Report(st) => Some((st.spec.clone(), st.original_name.clone())),
                _ => None,
            };
            if let Some((spec, old)) = payload {
                match spec.save(self.db.link()) {
                    Ok(()) => {
                        // A rename retires the old catalog row.
                        if let Some(old) = old.filter(|o| o != &spec.name) {
                            let _ = self.db.execute(&format!(
                                "DELETE FROM _phosphor_reports WHERE name = {}",
                                crate::store::q(&old)
                            ));
                        }
                        if let Overlay::Report(st) = &mut self.overlay {
                            st.original_name = Some(spec.name.clone());
                        }
                        self.say(format!(
                            "saved report {:?} (report {})",
                            spec.name, spec.name
                        ));
                    }
                    Err(e) => self.err(e),
                }
            }
        }
        if let Some(new) = app_rename {
            let old = match &self.overlay {
                Overlay::Apps(st) => Some(st.app.clone()),
                _ => None,
            };
            if let Some(old) = old {
                match appsgen::rename_app(self.db.link(), &old, &new) {
                    Ok(()) => {
                        if let Overlay::Apps(st) = &mut self.overlay {
                            st.app = new.clone();
                        }
                        self.say(format!("app renamed to {new:?}"));
                    }
                    Err(e) => self.err(e),
                }
            }
        }
    }

    fn source_columns_of_overlay(&self) -> Vec<String> {
        match &self.overlay {
            Overlay::Report(st) => self.source_columns(&st.spec),
            _ => Vec::new(),
        }
    }

    fn designer_run(&mut self) {
        // The table designer/editor's F2 is DDL; a readonly kiosk
        // never gets there.
        if self.readonly && matches!(self.overlay, Overlay::Create(_)) {
            return self.err("read-only mode: cannot change table structure");
        }
        match &self.overlay {
            Overlay::Qbe(st) => {
                let sql = st.spec.sql();
                self.overlay = Overlay::None;
                self.run_select(&sql);
            }
            Overlay::Report(st) => {
                let spec = st.spec.clone();
                match report::render(self.db.link(), &spec) {
                    Ok(lines) => {
                        self.overlay = Overlay::Pager(PagerState {
                            title: format!("REPORT · {}", spec.title),
                            lines,
                            offset: 0,
                            file_stem: format!("report_{}", spec.name),
                        })
                    }
                    Err(e) => self.err(e),
                }
            }
            Overlay::Apps(st) => {
                let app = st.app.clone();
                self.open_app_menu(Some(app));
            }
            Overlay::AppMenu(st) => {
                if let Some(item) = st.items.get(st.cursor) {
                    self.app_run_item(&item.clone());
                }
            }
            Overlay::Create(st) => {
                let draft = st.draft.clone();
                let schema = st.original.clone();
                match schema {
                    // TABLE EDITOR: apply the compiled script (ALTERs,
                    // or the rebuild — both transactional).
                    Some(schema) => {
                        let table = draft.table.clone();
                        // Capture index definitions BEFORE applying: a
                        // rebuild drops them with the old table.
                        let idx_sql: Vec<String> = self
                            .db
                            .query(&format!(
                                "SELECT sql FROM sqlite_master WHERE type = 'index' \
                                 AND tbl_name = {} AND sql IS NOT NULL",
                                crate::store::q(&schema.table)
                            ))
                            .map(|q| {
                                q.rows
                                    .iter()
                                    .filter_map(|r| match r.first() {
                                        Some(PValue::Text(t)) => Some(t.clone()),
                                        _ => None,
                                    })
                                    .collect()
                            })
                            .unwrap_or_default();
                        match draft.apply_script(&schema, &idx_sql) {
                            Ok(lines) if lines.is_empty() => {
                                self.say("no structural changes to apply");
                            }
                            Ok(lines) => {
                                let count = lines.len();
                                let sql = lines.join(";\n");
                                match self.db.execute(&sql) {
                                    Ok((_, elapsed)) => {
                                        self.last_ms = Some(elapsed.as_secs_f64() * 1000.0);
                                        self.overlay = Overlay::None;
                                        self.columns_cache.remove(&table);
                                        self.reload_tables();
                                        self.open_table(&table);
                                        self.say(format!(
                                            "applied {} change(s) to {table:?}",
                                            count
                                        ));
                                    }
                                    Err(e) => {
                                        // The batch aborted before COMMIT:
                                        // roll back so nothing partial
                                        // stays, and restore FK enforcement
                                        // (the rebuild toggles it OFF).
                                        let _ =
                                            self.db.execute("ROLLBACK; PRAGMA foreign_keys = ON");
                                        self.err(e);
                                    }
                                }
                            }
                            Err(e) => self.err(e),
                        }
                    }
                    None => match draft.create(self.db.link()) {
                        Ok(()) => {
                            self.overlay = Overlay::None;
                            self.reload_tables();
                            let table = draft.table.clone();
                            self.open_table(&table);
                            self.say(format!("created {table:?} — a adds the first record"));
                        }
                        Err(e) => self.err(e),
                    },
                }
            }
            Overlay::Form(_) => {
                // F2 in the list designer: enter the painter.
                if let Overlay::Form(st) = std::mem::replace(&mut self.overlay, Overlay::None) {
                    self.overlay = Overlay::Paint(PaintState::new(st.spec));
                }
            }
            _ => {}
        }
    }

    fn app_run_item(&mut self, item: &AppItem) {
        // In app mode, whatever this launches should come home to the
        // menu when it closes (found on film: Esc from a menu-launched
        // report stranded the user at the bare browser).
        self.menu_launched = self.app_home.is_some();
        match item.kind {
            ActionKind::Browse => {
                let table = item.action_ref.clone();
                self.overlay = Overlay::None;
                self.open_table(&table);
            }
            ActionKind::Query => match QbeSpec::saved_sql(self.db.link(), &item.action_ref) {
                Some(sql) => {
                    self.overlay = Overlay::None;
                    self.run_select(&sql);
                }
                None => self.err(format!(
                    "no saved query named {:?} (QBE F6 saves one)",
                    item.action_ref
                )),
            },
            ActionKind::Report => {
                let spec = ReportSpec::load(self.db.link(), &item.action_ref)
                    .unwrap_or_else(|| ReportSpec::for_table(&item.action_ref));
                match report::render(self.db.link(), &spec) {
                    Ok(lines) => {
                        self.overlay = Overlay::Pager(PagerState {
                            title: format!("REPORT · {}", spec.title),
                            lines,
                            offset: 0,
                            file_stem: format!("report_{}", spec.name),
                        })
                    }
                    Err(e) => self.err(e),
                }
            }
            ActionKind::Sql => {
                if self.readonly {
                    self.err("read-only mode: this action writes SQL");
                } else {
                    match self.db.execute(&item.action_ref) {
                        Ok((n, elapsed)) => {
                            self.last_ms = Some(elapsed.as_secs_f64() * 1000.0);
                            self.reload_tables();
                            self.say(match n {
                                -1 => "ok".to_owned(),
                                n => format!("ok, {n} row(s) affected"),
                            });
                        }
                        Err(e) => self.err(e),
                    }
                }
            }
            ActionKind::Script => {
                if self.readonly {
                    self.err("read-only mode: scripts can write");
                } else {
                    match crate::script::run(self.db.link(), &item.action_ref) {
                        Ok(out) => {
                            self.reload_tables();
                            self.refresh_health();
                            // A transcript is multi-line; the status bar
                            // is one line, so fold it.
                            let text = out.messages.join(" · ");
                            self.say(text);
                            self.apply_script_effects(&out.effects);
                        }
                        Err(e) => self.err(format!("script: {e}")),
                    }
                }
            }
        }
    }

    /// Turn queued `ui.*` effects into the same bus commands a keystroke
    /// would produce (DESIGN.md rule 1), so readonly and every other
    /// guard still applies.
    fn apply_script_effects(&mut self, effects: &[crate::script::Effect]) {
        use crate::script::Effect;
        for e in effects {
            let cmd = match e {
                Effect::Refresh => Command::Refresh,
                Effect::Prompt => Command::Focus(Focus::Prompt),
                Effect::Browse(t) => Command::OpenTable(t.clone()),
                Effect::Query(n) => Command::OpenSavedQuery(n.clone()),
                Effect::Report(n) => Command::OpenSavedReport(n.clone()),
                Effect::Form(t) => Command::OpenForm(Some(t.clone())),
                Effect::Quit => Command::Quit,
            };
            self.apply(cmd);
        }
    }

    fn designer_save(&mut self) {
        match &mut self.overlay {
            Overlay::Qbe(st) => {
                st.naming = true;
                st.editing = Some(String::new());
            }
            Overlay::Report(st) => {
                // F6 prompts for a name (prefilled), so reports can be
                // renamed and several can share a source (issue #8).
                st.naming = true;
                st.editing = Some(st.spec.name.clone());
                self.editor_fresh = true; // first keystroke replaces it
            }
            Overlay::Form(st) => {
                let spec = st.spec.clone();
                match spec.save(self.db.link()) {
                    Ok(()) => {
                        self.form_cache.remove(&spec.table);
                        self.say(format!(
                            "saved form for {:?} — EDIT uses it from now on",
                            spec.table
                        ))
                    }
                    Err(e) => self.err(e),
                }
            }
            Overlay::Paint(st) => {
                let spec = st.spec.clone();
                match spec.save(self.db.link()) {
                    Ok(()) => {
                        self.form_cache.remove(&spec.table);
                        self.say(format!(
                            "saved painted form for {:?} — EDIT renders it now",
                            spec.table
                        ))
                    }
                    Err(e) => self.err(e),
                }
            }
            _ => {}
        }
    }

    fn designer_add(&mut self) {
        if let Overlay::Create(st) = &mut self.overlay {
            // Insert AFTER the cursor row, dBASE-style (NAME row → top).
            let i = st.draft.insert_field(st.field_idx());
            st.cursor = i + 1;
            return;
        }
        if let Overlay::Apps(st) = &self.overlay {
            let app = st.app.clone();
            match appsgen::add_item(self.db.link(), &app, "New item") {
                Ok(()) => {
                    self.apps_reload(&app);
                    // Select the item just added: the next Enter must
                    // edit IT, not whatever the cursor was on.
                    if let Overlay::Apps(st) = &mut self.overlay {
                        st.cursor = st.items.len().saturating_sub(1);
                    }
                }
                Err(e) => self.err(e),
            }
        }
    }

    fn designer_delete(&mut self) {
        if let Overlay::Create(st) = &mut self.overlay {
            if let Some(i) = st.field_idx() {
                if i < st.draft.fields.len() {
                    st.draft.fields.remove(i);
                    st.cursor = st.cursor.min(st.draft.fields.len());
                }
            }
            return;
        }
        if let Overlay::Apps(st) = &self.overlay {
            let app = st.app.clone();
            if let Some(item) = st.items.get(st.cursor) {
                match appsgen::delete_item(self.db.link(), item.id) {
                    Ok(()) => self.apps_reload(&app),
                    Err(e) => self.err(e),
                }
            }
        }
    }

    fn designer_swap(&mut self, d: i64) {
        match &mut self.overlay {
            Overlay::Form(st) => {
                let n = st.spec.fields.len() as i64;
                let to = st.cursor as i64 + d;
                if to >= 0 && to < n {
                    st.spec.fields.swap(st.cursor, to as usize);
                    st.cursor = to as usize;
                }
            }
            Overlay::Paint(st) => {
                // +/- in the painter: widen/narrow the selected value cell.
                if let Some(f) = st.spec.fields.get_mut(st.selected) {
                    f.width = (f.width as i64 + d * 2).clamp(1, 60) as u16;
                }
            }
            Overlay::Create(st) => {
                if let Some(i) = st.field_idx() {
                    let to = i as i64 + d;
                    if to >= 0 && (to as usize) < st.draft.fields.len() {
                        st.draft.fields.swap(i, to as usize);
                        st.cursor = to as usize + 1;
                    }
                }
            }
            Overlay::Apps(st) => {
                let to = st.cursor as i64 + d;
                if to >= 0 && (to as usize) < st.items.len() {
                    let (a, b) = (st.items[st.cursor].clone(), st.items[to as usize].clone());
                    let app = st.app.clone();
                    let cursor_to = to as usize;
                    match appsgen::swap_items(self.db.link(), &a, &b) {
                        Ok(()) => {
                            self.apps_reload(&app);
                            if let Overlay::Apps(st) = &mut self.overlay {
                                st.cursor = cursor_to;
                            }
                        }
                        Err(e) => self.err(e),
                    }
                }
            }
            _ => {}
        }
    }

    fn apps_reload(&mut self, app: &str) {
        let items = appsgen::items(self.db.link(), app);
        if let Overlay::Apps(st) = &mut self.overlay {
            st.items = items;
            st.cursor = st.cursor.min(st.items.len().saturating_sub(1));
        }
    }

    fn open_form(&mut self, table: Option<String>) {
        let Some(table) = self.target_table(table) else {
            return self.err("form: no table selected (form <table>)");
        };
        let spec = match self.cached_form(&table) {
            Some(spec) => Ok(spec),
            // Miss: build the default from (cached) columns — one DB
            // hit total instead of load-probe + columns query.
            None => match self.cached_columns(&table) {
                Ok(cols) => Ok(FormSpec::from_columns(&table, cols)),
                Err(e) => Err(e),
            },
        };
        match spec {
            Ok(spec) => {
                self.overlay = Overlay::Form(FormState {
                    spec,
                    cursor: 0,
                    editing: None,
                    editing_mask: false,
                    editing_computed: false,
                })
            }
            Err(e) => self.err(e),
        }
    }

    fn open_apps(&mut self, name: Option<String>) {
        let name = name
            .or_else(|| appsgen::list_apps(self.db.link()).into_iter().next())
            .unwrap_or_else(|| "app".to_owned());
        // Deliberately no ensure here: opening the designer is a READ.
        // The first DesignerAdd creates the app (and its tables).
        let items = appsgen::items(self.db.link(), &name);
        self.overlay = Overlay::Apps(AppDesignState {
            app: name,
            items,
            cursor: 0,
            editing: None,
            editing_ref: false,
            renaming_app: false,
        });
    }

    /// `r` in the Applications Generator: type the app's name.
    fn rename_app_begin(&mut self) {
        self.editor_fresh = true;
        if let Overlay::Apps(st) = &mut self.overlay {
            st.renaming_app = true;
            st.editing_ref = false;
            st.editing = Some(st.app.clone());
        }
    }

    fn open_app_menu(&mut self, name: Option<String>) {
        let Some(name) = name.or_else(|| appsgen::list_apps(self.db.link()).into_iter().next())
        else {
            return self.err("no apps in this database yet — press A to craft one");
        };
        let items = appsgen::items(self.db.link(), &name);
        if items.is_empty() {
            return self.err(format!("app {name:?} has no items yet — A to design"));
        }
        let version = appsgen::app_version(self.db.link(), &name);
        self.overlay = Overlay::AppMenu(AppMenuState {
            app: name,
            version,
            items,
            cursor: 0,
        });
    }

    /// Run a SELECT into the query grid (shared by prompt + QBE + apps).
    fn run_select(&mut self, sql: &str) {
        // Async: arbitrary user SQL can take arbitrarily long (remote
        // analytical queries especially). The grid swaps in on arrival;
        // until then the previous screen stays put.
        let seq = self.status_seq;
        let query = sql.to_owned();
        match self
            .db
            .submit(Box::new(move |db| DbResponse::Query(db.query(&query))))
        {
            Some(tag) => {
                self.pending.insert(tag, PendingOp::Select { seq });
            }
            None => self.err("database worker is gone"),
        }
    }

    fn open_selected(&mut self) {
        if let Some(t) = self.visible_tables().get(self.sidebar_idx) {
            let name = t.name.clone();
            self.open_table(&name);
        }
    }

    // ── split BROWSE: SET RELATION on one screen ─────────────────────

    /// 'v' in BROWSE: open/cycle/close the detail pane. The pane shows
    /// the child rows of the master cursor's record (declared FKs only,
    /// same discovery as the EDIT link panes). Needs a wide terminal —
    /// narrow screens keep the single-pane layout they're good at.
    fn toggle_split(&mut self) {
        let parent = match &self.grid {
            Some(g) => match &g.source {
                GridSource::Table { name, .. } => name.clone(),
                _ => {
                    return self.say("split works on a table BROWSE");
                }
            },
            None => return self.say("open a table first (Enter in the sidebar)"),
        };
        // Already open: advance along the FROZEN cycle (stored on the
        // pane), or close after the last one. The stored order is what
        // navigation follows — recomputing per press would reorder
        // under the user (cycling updates the remembered pref).
        if self.detail.is_some() {
            let cycle = self.detail.as_ref().unwrap().cycle.clone();
            let current = self.detail.as_ref().and_then(|d| match &d.grid.source {
                GridSource::Detail {
                    child, child_col, ..
                } => Some((child.clone(), child_col.clone())),
                _ => None,
            });
            let pos = cycle.iter().position(|l| {
                current
                    .as_ref()
                    .is_some_and(|(c, k)| c == &l.0 && k == &l.1)
            });
            if let Some(next) = pos.and_then(|i| cycle.get(i + 1)) {
                let (child, col, pcol) = next.clone();
                let cyc = cycle.clone();
                self.open_detail(child, col, pcol, Some(cyc));
            } else {
                self.close_detail();
            }
            return;
        }
        // Opening: side-by-side needs width; stacked needs height.
        if self.split_horizontal {
            if self.visible_rows < 8 {
                return self.say("stacked split needs more rows");
            }
        } else if self.visible_cols_width < 74 {
            return self.say("split view needs a wider terminal (100+ cols)");
        }
        // The cycle: the remembered link fronts it, the rest follow in
        // declaration order — a past choice reorders, never hides. v
        // walks cycle[0] → cycle[1] → … → close → cycle[0]…
        let links = self.cached_links(&parent);
        let remembered =
            store::pref_get(self.db.link(), &format!("split:{parent}")).and_then(|v| {
                let (c, k) = v.split_once('\u{1}')?;
                links
                    .iter()
                    .find(|l| l.0.eq_ignore_ascii_case(c) && l.1.eq_ignore_ascii_case(k))
                    .cloned()
            });
        let mut cycle = links.clone();
        if let Some(r) = &remembered {
            if let Some(i) = cycle
                .iter()
                .position(|l| l.0.eq_ignore_ascii_case(&r.0) && l.1.eq_ignore_ascii_case(&r.1))
            {
                let r = cycle.remove(i);
                cycle.insert(0, r);
            }
        }
        match cycle.first() {
            Some((child, col, pcol)) => {
                let (c, k, p) = (child.clone(), col.clone(), pcol.clone());
                let cyc = cycle.clone();
                self.open_detail(c, k, p, Some(cyc));
            }
            None => self.say(format!(
                "{parent} has no related tables (declared foreign keys)"
            )),
        }
    }

    /// (Re)target the detail pane at (child, child_col) for the current
    /// master record. Installs a placeholder immediately; rows fly in
    /// async and swap when they match the still-current key.
    /// `cycle` freezes the 'v' navigation order for this pane's life.
    fn open_detail(
        &mut self,
        child: String,
        child_col: String,
        parent_col: String,
        cycle: Option<Vec<(String, String, String)>>,
    ) {
        let Some(key_sql) = self.master_key_sql(&parent_col) else {
            return self.say("this record has no key to relate on");
        };
        let parent = match &self.grid {
            Some(g) => match &g.source {
                GridSource::Table { name, .. } => name.clone(),
                _ => return,
            },
            None => return,
        };
        let grid = Grid {
            source: GridSource::Detail {
                parent: parent.clone(),
                child: child.clone(),
                child_col: child_col.clone(),
                key_sql: key_sql.clone(),
            },
            columns: Vec::new(),
            total: 0,
            cache: Vec::new(),
            cache_start: 0,
            rowids: None,
            cur_row: 0,
            cur_col: 0,
            row_off: 0,
            col_off: 0,
            widths: Vec::new(),
        };
        self.detail = Some(DetailState {
            grid,
            parent_col,
            visible_rows: 12,
            cycle: cycle.unwrap_or_default(),
        });
        // Remember the layout: reopening this table restores its split.
        store::pref_set(
            self.db.link(),
            &format!("split:{parent}"),
            &format!("{child}\u{1}{child_col}"),
        );
        self.submit_detail(&child, &child_col, &key_sql);
    }

    fn close_detail(&mut self) {
        self.detail = None;
        self.pending_detail = None;
        if self.focus == Focus::Detail {
            self.focus = Focus::Grid;
        }
    }

    /// The master cursor's key for `parent_col` as a SQL literal —
    /// same resolution as the EDIT link panes (named column, else pk,
    /// else rowid).
    fn master_key_sql(&self, parent_col: &str) -> Option<String> {
        let g = self.grid.as_ref()?;
        let table = match &g.source {
            GridSource::Table { name, .. } => name.as_str(),
            _ => return None,
        };
        let idx = g.cur_row.checked_sub(g.cache_start)? as usize;
        let row = g.cache.get(idx)?;
        // Named FK column first (ASCII-case-insensitive, like the
        // link panes).
        if !parent_col.is_empty() {
            let cols = self.columns_cache.get(table)?;
            if let Some((i, _)) = cols
                .iter()
                .enumerate()
                .find(|(_, c)| c.name.eq_ignore_ascii_case(parent_col))
            {
                return match row.get(i)? {
                    PValue::Null => None,
                    PValue::Int(i) => Some(i.to_string()),
                    PValue::Real(r) => Some(r.to_string()),
                    v => Some(format!("'{}'", v.render().replace('\'', "''"))),
                };
            }
        }
        // pk fallback (an INTEGER PRIMARY KEY *is* the rowid), then rowid.
        let pk_is_first = self
            .columns_cache
            .get(table)
            .and_then(|cols| cols.first())
            .is_some_and(|c| c.pk);
        match (
            pk_is_first,
            row.first(),
            g.rowids.as_ref().and_then(|r| r.get(idx)),
        ) {
            (true, Some(PValue::Int(id)), _) => Some(id.to_string()),
            (_, _, Some(id)) => Some(id.to_string()),
            _ => None,
        }
    }

    /// Submit the detail fetch: filtered count + rows in ONE worker job.
    /// Latest-wins via pending_detail; a stale arrival (master moved on)
    /// is dropped by the finish arm.
    fn submit_detail(&mut self, child: &str, child_col: &str, key_sql: &str) {
        let (child, child_col, key_sql) =
            (child.to_owned(), child_col.to_owned(), key_sql.to_owned());
        let limit = 2001; // bounded fetch; count reports the real total
        let want_key = key_sql.to_owned();
        match self.db.submit(Box::new(move |db| {
            let where_clause = format!("\"{}\" = {}", child_col.replace('"', "\"\""), key_sql);
            let order = if db.has_rowid(&child) {
                " ORDER BY rowid"
            } else {
                ""
            };
            let q = db.query(&format!(
                "SELECT * FROM \"{}\" WHERE {}{} LIMIT {}",
                child.replace('"', "\"\""),
                where_clause,
                order,
                limit
            ));
            let total = db.query(&format!(
                "SELECT count(*) FROM \"{}\" WHERE {}",
                child.replace('"', "\"\""),
                where_clause
            ));
            DbResponse::Detail(match (q, total) {
                (Ok(q), Ok(t)) => Ok(crate::worker::DetailData {
                    columns: q.columns,
                    rows: q.rows,
                    total: t
                        .rows
                        .first()
                        .and_then(|r| r.first())
                        .map_or(0, |v| match v {
                            PValue::Int(n) => *n,
                            _ => 0,
                        }),
                }),
                (Err(e), _) | (_, Err(e)) => Err(e),
            })
        })) {
            Some(tag) => {
                self.pending.insert(
                    tag,
                    PendingOp::Detail {
                        want_key: want_key.clone(),
                    },
                );
                self.pending_detail = Some((tag, want_key));
            }
            None => self.err("database worker is gone"),
        }
    }

    /// Master cursor moved (or values changed): re-link the detail pane.
    fn refresh_detail(&mut self) {
        let Some(state) = &self.detail else { return };
        let (child, child_col, key_sql) = match &state.grid.source {
            GridSource::Detail {
                child,
                child_col,
                key_sql,
                ..
            } => (child.clone(), child_col.clone(), key_sql.clone()),
            _ => return,
        };
        let Some(new_key) = self.master_key_sql(&state.parent_col) else {
            return; // unkeyed record: pane keeps its last picture
        };
        if key_sql == new_key {
            return; // same record, nothing to re-link
        }
        let state = self.detail.as_mut().unwrap();
        if let GridSource::Detail { key_sql, .. } = &mut state.grid.source {
            *key_sql = new_key.clone();
        }
        self.submit_detail(&child, &child_col, &new_key);
    }

    /// Detail-pane local navigation (its own grid, no cascade).
    fn detail_move(&mut self, dr: i64, dc: i64) {
        let visible = self
            .detail
            .as_ref()
            .map(|d| d.visible_rows.max(1))
            .unwrap_or(1);
        let Some(state) = &mut self.detail else {
            return;
        };
        let g = &mut state.grid;
        if g.total == 0 {
            return;
        }
        g.cur_row = (g.cur_row + dr).clamp(0, g.total - 1);
        g.cur_col = (g.cur_col as i64 + dc).clamp(0, g.columns.len() as i64 - 1) as usize;
        if g.cur_row < g.row_off {
            g.row_off = g.cur_row;
        }
        if g.cur_row >= g.row_off + visible {
            g.row_off = g.cur_row - visible + 1;
        }
        while g.col_off < g.cur_col {
            let used: u16 = g.widths[g.col_off..=g.cur_col].iter().map(|w| w + 1).sum();
            if used <= self.visible_cols_width / 2 {
                break;
            }
            g.col_off += 1;
        }
    }

    fn detail_jump(&mut self, row: i64) {
        if let Some(state) = &mut self.detail {
            state.grid.cur_row = row.clamp(0, state.grid.total.saturating_sub(1).max(0));
        }
        self.detail_move(0, 0);
    }

    // ── mouse: hit-testing → the same bus commands as keys ───────────

    fn contains(r: Option<ratatui::layout::Rect>, col: u16, row: u16) -> bool {
        r.is_some_and(|r| col >= r.x && col < r.x + r.width && row >= r.y && row < r.y + r.height)
    }

    /// Route a left-click / wheel event through the command bus.
    /// Click selects; clicking the sidebar selection again opens it;
    /// clicking the master's cursor row again opens EDIT.
    pub fn on_mouse(
        &mut self,
        kind: &ratatui::crossterm::event::MouseEventKind,
        col: u16,
        row: u16,
    ) {
        use ratatui::crossterm::event::MouseEventKind as K;
        match kind {
            K::ScrollUp | K::ScrollDown => {
                let d: i64 = if matches!(kind, K::ScrollUp) { -1 } else { 1 };
                if Self::contains(self.hit.master, col, row)
                    || Self::contains(self.hit.detail, col, row)
                {
                    self.apply(Command::GridScroll(d));
                }
            }
            K::Down(_) => {
                if Self::contains(self.hit.sidebar, col, row) {
                    let idx = (row - self.hit.sidebar.unwrap().y) as usize;
                    self.apply(Command::SidebarClick(idx));
                } else if Self::contains(self.hit.master, col, row) {
                    let r = self.hit.master.unwrap();
                    self.apply(Command::GridClick {
                        row: self.grid.as_ref().map_or(0, |g| g.row_off) + (row - r.y - 1) as i64,
                    });
                } else if Self::contains(self.hit.detail, col, row) {
                    self.apply(Command::Focus(Focus::Detail));
                    let r = self.hit.detail.unwrap();
                    let off = self.detail.as_ref().map_or(0, |d| d.grid.row_off);
                    self.apply(Command::GridClick {
                        row: off + (row - r.y - 1) as i64,
                    });
                } else if Self::contains(self.hit.prompt, col, row) {
                    self.apply(Command::Focus(Focus::Prompt));
                }
            }
            _ => {}
        }
    }

    fn click_target(&self) -> Option<&'static str> {
        if self.focus == Focus::Detail {
            Some("detail")
        } else if self.focus == Focus::Grid {
            Some("grid")
        } else if self.focus == Focus::Sidebar {
            Some("sidebar")
        } else {
            None
        }
    }

    fn open_table(&mut self, name: &str) {
        // Async: columns + rowid-ness + first window + total bundle into
        // ONE worker job (one round-trip, cold or warm). The previous
        // grid stays on screen until the new one swaps in — no flash,
        // and failures leave the old view intact.
        let table = name.to_owned();
        let limit = self.visible_rows + OVERSCAN;
        let seq = self.status_seq;
        let submitted = self.db.submit(Box::new(move |db| {
            let res = (|| -> DbResult<crate::worker::OpenedGrid> {
                let columns = db.columns(&table)?;
                let editable = db.has_rowid(&table);
                let (page, total) = db.open_window(&table, 0, limit)?;
                Ok(crate::worker::OpenedGrid {
                    columns,
                    editable,
                    page,
                    total,
                })
            })();
            DbResponse::Opened(res)
        }));
        match submitted {
            Some(tag) => {
                self.pending.insert(
                    tag,
                    PendingOp::Open {
                        name: name.to_owned(),
                        seq,
                    },
                );
            }
            None => self.err("database worker is gone"),
        }
    }

    fn grid_jump(&mut self, row: i64) {
        if let Some(g) = &mut self.grid {
            g.cur_row = row.clamp(0, g.total.saturating_sub(1).max(0));
        }
        self.grid_move(0, 0);
    }

    fn grid_move(&mut self, dr: i64, dc: i64) {
        let visible = self.visible_rows.max(1);
        let Some(g) = &mut self.grid else { return };
        if g.total == 0 {
            return;
        }
        g.cur_row = (g.cur_row + dr).clamp(0, g.total - 1);
        g.cur_col = (g.cur_col as i64 + dc).clamp(0, g.columns.len() as i64 - 1) as usize;
        if g.cur_row < g.row_off {
            g.row_off = g.cur_row;
        }
        if g.cur_row >= g.row_off + visible {
            g.row_off = g.cur_row - visible + 1;
        }
        // Horizontal: slide col_off until the cursor column fits.
        if g.cur_col < g.col_off {
            g.col_off = g.cur_col;
        }
        while g.col_off < g.cur_col {
            let used: u16 = g.widths[g.col_off..=g.cur_col].iter().map(|w| w + 1).sum();
            if used <= self.visible_cols_width {
                break;
            }
            g.col_off += 1;
        }
        self.ensure_cache();
        // Cursor-linked detail: every master movement re-points the
        // split pane (single choke point — commands, find, refill and
        // the insert flip all funnel through here).
        self.refresh_detail();
    }

    /// Virtualization: keep [row_off-OVERSCAN, row_off+visible+OVERSCAN)
    /// cached for Table sources. Query sources are fully materialized.
    /// Async: a missing window submits one Page job and returns; the
    /// renderer shows "…" placeholders until it installs. Rapid scrolls
    /// supersede (latest token wins), duplicates never re-submit.
    fn ensure_cache(&mut self) {
        let visible = self.visible_rows.max(1);
        let Some(g) = &self.grid else { return };
        let GridSource::Table { name, .. } = &g.source else {
            return;
        };
        let want_start = (g.row_off - OVERSCAN).max(0);
        let want_end = (g.row_off + visible + OVERSCAN).min(g.total);
        let have_start = g.cache_start;
        let have_end = g.cache_start + g.cache.len() as i64;
        if want_start >= have_start && want_end <= have_end {
            return;
        }
        let name = name.clone();
        let limit = want_end - want_start;
        if let Some((_, t, s)) = &self.pending_page {
            if *t == name && *s == want_start {
                return; // already flying; its arrival installs it
            }
        }
        let job_name = name.clone();
        match self.db.submit(Box::new(move |db| {
            DbResponse::Page(db.page(&job_name, want_start, limit))
        })) {
            Some(tag) => {
                self.pending.insert(
                    tag,
                    PendingOp::Page {
                        table: name.clone(),
                        want_start,
                    },
                );
                self.pending_page = Some((tag, name, want_start));
            }
            None => self.err("database worker is gone"),
        }
    }

    /// Force-submit the live window (post-write truth): like ensure
    /// but bypasses coverage (values changed) and supersedes any
    /// pre-write window still flying. The stale grid stays until swap.
    fn refresh_window(&mut self) {
        let visible = self.visible_rows.max(1);
        let Some(g) = &self.grid else { return };
        let GridSource::Table { name, .. } = &g.source else {
            return;
        };
        let want_start = (g.row_off - OVERSCAN).max(0);
        let want_end = (g.row_off + visible + OVERSCAN).min(g.total);
        let name = name.clone();
        let limit = (want_end - want_start).max(1);
        let job_name = name.clone();
        match self.db.submit(Box::new(move |db| {
            DbResponse::Page(db.page(&job_name, want_start, limit))
        })) {
            Some(tag) => {
                self.pending.insert(
                    tag,
                    PendingOp::Page {
                        table: name.clone(),
                        want_start,
                    },
                );
                self.pending_page = Some((tag, name, want_start));
            }
            None => self.err("database worker is gone"),
        }
    }

    fn open_edit(&mut self) {
        match &self.grid {
            Some(Grid {
                source: GridSource::Table { editable, .. },
                ..
            }) => {
                if !*editable {
                    return self.say("this table has no rowid; BROWSE is read-only here");
                }
            }
            Some(_) => return self.say("query results are read-only (Esc to go back)"),
            None => return,
        }
        let abs = self.grid.as_ref().map(|g| g.cur_row).unwrap_or(0);
        self.build_edit_for(abs);
    }

    /// Build a parked EDIT target once a window install covers it.
    /// Called after every cache install (Page/Refill arrivals).
    fn try_pending_edit(&mut self) {
        let Some(abs) = self.pending_edit.take() else {
            return;
        };
        // The table may have shrunk under the parked row: clamp first
        // so a stale target can't re-park forever.
        let abs = match &self.grid {
            Some(g) => abs.clamp(0, g.total.saturating_sub(1).max(0)),
            None => return, // grid gone; forget the parked edit
        };
        self.build_edit_for(abs); // re-parks itself if still uncovered
    }

    /// Synchronous flip onto the tail record after an INSERT:
    /// positions the cursor, fetches the tail window blocking, installs
    /// it, and builds the form — all before returning, so a second save
    /// cannot insert twins. Rare path (explicit save keypress); bulk
    /// flight stays async.
    fn flip_to_tail(&mut self, last: i64, new_rowid: i64) {
        let visible = self.visible_rows.max(1);
        let name = match &self.grid {
            Some(g) => match &g.source {
                GridSource::Table { name, .. } => name.clone(),
                _ => return,
            },
            None => return,
        };
        if let Some(g) = &mut self.grid {
            g.cur_row = last.clamp(0, g.total.saturating_sub(1).max(0));
            if g.cur_row < g.row_off {
                g.row_off = g.cur_row;
            }
            if g.cur_row >= g.row_off + visible {
                g.row_off = g.cur_row - visible + 1;
            }
        }
        let (want_start, limit) = match &self.grid {
            Some(g) => {
                let s = (g.row_off - OVERSCAN).max(0);
                (s, (g.row_off + visible + OVERSCAN).min(g.total) - s)
            }
            None => return,
        };
        match self.db.page(&name, want_start, limit.max(1)) {
            Ok(page) => {
                if let Some(g) = &mut self.grid {
                    g.cache = page.rows;
                    g.rowids = page.rowids;
                    g.cache_start = want_start;
                    if g.widths.is_empty() {
                        g.compute_widths();
                    }
                }
                // Void in-flight windows: their data predates the insert.
                self.pending_page = None;
            }
            Err(e) => {
                // No twin window on ANY path: demote the form from
                // INSERT to UPDATE against the new rowid, so further
                // Enters edit the inserted record instead of inserting
                // twins while the fetch is broken.
                self.pending_page = None;
                if let Overlay::Edit(ed) = &mut self.overlay {
                    ed.inserting = false;
                    ed.rowid = new_rowid;
                }
                return self.err(e);
            }
        }
        let abs = self.grid.as_ref().map(|g| g.cur_row).unwrap_or(0);
        self.build_edit_for(abs);
        // If the form build couldn't complete (fetch/columns failure),
        // still demote — same no-twins contract.
        if let Overlay::Edit(ed) = &mut self.overlay {
            if ed.inserting {
                ed.inserting = false;
                ed.rowid = new_rowid;
            }
        }
    }

    /// Build (or rebuild) the EDIT overlay for the record at absolute
    /// grid row `abs` — used by open_edit and by record PAGING.
    /// Async-aware: grid_jump submits a missing window; if its rows
    /// aren't here yet the target parks in pending_edit and the form
    /// builds when a window install covers it (try_pending_edit).
    fn build_edit_for(&mut self, abs: i64) {
        // Make sure the cache covers the target row, and move the grid
        // cursor with the form so context follows the flip.
        self.grid_jump(abs);
        let Some(g) = &self.grid else { return };
        let GridSource::Table { name, .. } = &g.source else {
            return;
        };
        let idx = abs - g.cache_start;
        let (Some(row), Some(rowids)) = (g.row(abs), &g.rowids) else {
            self.pending_edit = Some(abs);
            return;
        };
        let Some(rowid) = rowids.get(idx as usize).copied() else {
            return;
        };
        self.pending_edit = None; // rows present: any parked target is served
        let name = name.clone();
        let row: Vec<PValue> = row.clone();
        let cols = match self.cached_columns(&name) {
            Ok(c) => c,
            Err(e) => return self.err(e),
        };
        let mut fields: Vec<(ColumnInfo, PValue)> = cols.into_iter().zip(row).collect();
        let mut labels: Vec<String> = fields.iter().map(|(c, _)| c.name.clone()).collect();
        let mut required: Vec<bool> = vec![false; fields.len()];
        let mut masks: Vec<String> = vec![String::new(); fields.len()];
        let mut computed: Vec<Option<String>> = vec![None; fields.len()];

        // A crafted form (phase 5) reorders, relabels, hides, requires,
        // masks, and adds computed columns; a PAINTED one also brings its
        // 2D layout.
        let painted = self.apply_crafted_form(
            &name,
            &mut fields,
            &mut labels,
            &mut required,
            &mut masks,
            &mut computed,
        );
        // Derived values are read-only; compute them for this record.
        self.eval_computed(&name, rowid, &computed, &mut fields);

        let n = fields.len();
        let links = self.build_link_panes(&name, &fields, rowid);
        let pickers = self.build_pickers(&name, &fields);
        // Preserve the field cursor across a page flip.
        let keep_cursor = match &self.overlay {
            Overlay::Edit(prev) if !prev.inserting => prev.cursor.min(n.saturating_sub(1)),
            _ => 0,
        };
        self.overlay = Overlay::Edit(EditState {
            table: name,
            inserting: false,
            painted,
            row_abs: abs,
            rowid,
            fields,
            labels,
            required,
            masks,
            computed,
            inputs: vec![None; n],
            cursor: keep_cursor,
            editing: None,
            links,
            pickers,
            picker: None,
        });
    }

    /// For each field, the FK parent a picker would read from (F7), or
    /// None for ordinary columns. One cached introspection per table,
    /// then a pk fallback for bare `REFERENCES parent`.
    fn build_pickers(
        &mut self,
        table: &str,
        fields: &[(ColumnInfo, PValue)],
    ) -> Vec<Option<(String, String)>> {
        let fks = self.cached_outgoing_fks(table);
        let mut out = Vec::with_capacity(fields.len());
        for (c, _) in fields {
            let hit = fks
                .iter()
                .find(|(from, _, _)| from.eq_ignore_ascii_case(&c.name))
                .cloned();
            let resolved = match hit {
                None => None,
                Some((_, to_table, to_col)) => {
                    let key = match to_col {
                        Some(col) => col,
                        None => self
                            .cached_columns(&to_table)
                            .ok()
                            .and_then(|cs| cs.iter().find(|c| c.pk).map(|c| c.name.clone()))
                            .unwrap_or_else(|| "rowid".into()),
                    };
                    Some((to_table, key))
                }
            };
            out.push(resolved);
        }
        out
    }

    /// F7: open the value picker for the field under the cursor when it
    /// is a declared foreign key; list the parent rows and let the user
    /// pick one. Looks up `(parent, key column) -> rows`.
    fn open_picker(&mut self) {
        let (parent, keycol, field) = match &self.overlay {
            Overlay::Edit(ed) => match ed.pickers.get(ed.cursor) {
                Some(Some((t, k))) => (t.clone(), k.clone(), ed.cursor),
                _ => {
                    return self.say("F7 picks values for foreign-key fields only");
                }
            },
            _ => return,
        };
        let sql = format!(
            "SELECT {} AS \"{}\", * FROM {} ORDER BY {} LIMIT 200",
            Self::quote_ident(&keycol),
            keycol,
            Self::quote_ident(&parent),
            Self::quote_ident(&keycol)
        );
        match self.db.query(&sql) {
            Ok(q) => {
                if q.rows.is_empty() {
                    return self.say(format!("{parent} has no rows to pick"));
                }
                if let Overlay::Edit(ed) = &mut self.overlay {
                    ed.picker = Some(PickerState {
                        title: format!(" PICK · {parent} "),
                        columns: q.columns,
                        rows: q.rows,
                        cursor: 0,
                        field,
                        key_col: 0,
                    });
                }
            }
            Err(e) => self.err(e),
        }
    }

    /// SET RELATION, reborn: one pane per declared FK pointing at this
    /// table, filtered to the record on screen. Refreshed per page flip.
    fn build_link_panes(
        &mut self,
        parent: &str,
        fields: &[(ColumnInfo, PValue)],
        rowid: i64,
    ) -> Vec<LinkPane> {
        let mut out = Vec::new();
        // Column index once (was a linear find per pane per flip).
        let by_name: HashMap<&str, &PValue> =
            fields.iter().map(|(c, v)| (c.name.as_str(), v)).collect();
        let pk_val = fields.iter().find(|(c, _)| c.pk).map(|(_, v)| v);
        for (child, child_col, parent_col) in self.cached_links(parent) {
            // The parent-side key: the named column, or the pk (whose
            // value for an INTEGER PRIMARY KEY is the rowid itself).
            let key = if parent_col.is_empty() {
                pk_val.cloned().unwrap_or(PValue::Int(rowid))
            } else if let Some(v) = by_name.get(parent_col.as_str()) {
                (*v).clone()
            } else {
                // Case-variant fallback (same semantics as before).
                fields
                    .iter()
                    .find(|(c, _)| c.name.eq_ignore_ascii_case(&parent_col))
                    .map(|(_, v)| v.clone())
                    .unwrap_or(PValue::Int(rowid))
            };
            let key_sql = match &key {
                PValue::Null => continue, // unsaved/keyless: no pane
                PValue::Int(i) => i.to_string(),
                PValue::Real(r) => r.to_string(),
                v => format!("'{}'", v.render().replace('\'', "''")),
            };
            // Revisited keys reuse the cached pane (no queries at all).
            let cache_key = (child.clone(), child_col.clone(), key_sql.clone());
            if let Some(pane) = self.pane_cache.get(&cache_key) {
                out.push(pane.clone());
                continue;
            }
            let Some(pane) = self.fetch_pane(&child, &child_col, &key_sql) else {
                continue;
            };
            // Cap the cache: a cross-country flight over millions of
            // rows must not pin millions of previews.
            if self.pane_cache.len() >= 512 {
                self.pane_cache.clear();
            }
            self.pane_cache.insert(cache_key, pane.clone());
            out.push(pane);
        }
        out
    }

    /// One pane's preview + total in a SINGLE query: count(*) OVER ()
    /// rides along with the LIMITed preview rows (was count(*) + SELECT
    /// = 2 round-trips per pane per flip). Falls back to the two-query
    /// form on engines without window functions.
    fn fetch_pane(&self, child: &str, child_col: &str, key_sql: &str) -> Option<LinkPane> {
        const PREVIEW_ROWS: usize = 4;
        const PREVIEW_COLS: usize = 4;
        let qchild = child.replace('"', "\"\"");
        let qcol = child_col.replace('"', "\"\"");
        if let Ok(q) = self.db.query(&format!(
            "SELECT *, count(*) OVER () AS _pane_total FROM \"{qchild}\" \
             WHERE \"{qcol}\" = {key_sql} LIMIT {PREVIEW_ROWS}"
        )) {
            // Our appended total is the LAST column; strip it back off.
            if q.columns.last().is_some_and(|c| c == "_pane_total") {
                let n = q.columns.len() - 1;
                let total = q
                    .rows
                    .first()
                    .and_then(|r| r.get(n))
                    .and_then(|v| match v {
                        PValue::Int(t) => Some(*t),
                        _ => None,
                    })
                    .unwrap_or(0);
                // Preview the first few NON-key columns — the fk value is
                // already on the parent form; show what's interesting.
                let keep: Vec<usize> = (0..n)
                    .filter(|&i| !q.columns[i].eq_ignore_ascii_case(child_col))
                    .take(PREVIEW_COLS)
                    .collect();
                return Some(LinkPane {
                    header: keep.iter().map(|&i| q.columns[i].clone()).collect(),
                    rows: q
                        .rows
                        .iter()
                        .map(|r| keep.iter().map(|&i| r[i].render()).collect())
                        .collect(),
                    total,
                    key_sql: key_sql.to_owned(),
                    child: child.to_owned(),
                    child_col: child_col.to_owned(),
                });
            }
        }
        // Fallback: count + preview as two queries.
        let total = self
            .db
            .query(&format!(
                "SELECT count(*) FROM \"{qchild}\" WHERE \"{qcol}\" = {key_sql}"
            ))
            .ok()
            .and_then(|q| match q.rows.first().and_then(|r| r.first()) {
                Some(PValue::Int(n)) => Some(*n),
                _ => None,
            })
            .unwrap_or(0);
        let q = self
            .db
            .query(&format!(
                "SELECT * FROM \"{qchild}\" WHERE \"{qcol}\" = {key_sql} LIMIT {PREVIEW_ROWS}"
            ))
            .ok()?;
        let keep: Vec<usize> = (0..q.columns.len())
            .filter(|&i| !q.columns[i].eq_ignore_ascii_case(child_col))
            .take(PREVIEW_COLS)
            .collect();
        Some(LinkPane {
            header: keep.iter().map(|&i| q.columns[i].clone()).collect(),
            rows: q
                .rows
                .iter()
                .map(|r| keep.iter().map(|&i| r[i].render()).collect())
                .collect(),
            total,
            key_sql: key_sql.to_owned(),
            child: child.to_owned(),
            child_col: child_col.to_owned(),
        })
    }

    /// PgUp/PgDn (or ←→) in EDIT: flip to the previous/next RECORD,
    /// dBASE-style — dirty edits COMMIT as you page (validation holds
    /// the page instead). The form stays open; hold the key and fly.
    fn edit_page(&mut self, d: i64) {
        self.fold_editing_buffer();
        let (inserting, dirty, from) = match &self.overlay {
            Overlay::Edit(ed) => (ed.inserting, ed.dirty(), ed.row_abs),
            _ => return,
        };
        if inserting {
            return self.say("save the new record first (F10), then page");
        }
        // Sequence from the parked target when a flip is still flying
        // in: rapid holds would otherwise recompute from the stale form
        // and skip the parked record.
        let from = self.pending_edit.unwrap_or(from);
        // Held-key acceleration: rapid repeats stretch the stride, so
        // holding PgDn goes from record-at-a-time to 10-at-a-time —
        // key autorepeat (~25/s) stops being the speed limit.
        let now = std::time::Instant::now();
        let rapid = self
            .last_edit_page
            .is_some_and(|t| now.duration_since(t) < std::time::Duration::from_millis(150));
        self.page_streak = if rapid { self.page_streak + 1 } else { 0 };
        self.last_edit_page = Some(now);
        let step = (1 + self.page_streak as i64 / 6).min(10);

        let total = self.grid.as_ref().map(|g| g.total).unwrap_or(0);
        let target = (from + d * step).clamp(0, total.saturating_sub(1).max(0));
        if target == from {
            return self.say(if d < 0 { "first record" } else { "last record" });
        }
        if dirty && !self.commit_edit() {
            return; // validation or db error: stay on this record
        }
        self.build_edit_for(target);
    }

    /// Apply the saved form for `table` (order/labels/hide/required) to
    /// the parallel field vectors; returns the spec when it is PAINTED
    /// so EDIT can render the 2D layout.
    #[allow(clippy::too_many_arguments)]
    fn apply_crafted_form(
        &mut self,
        table: &str,
        fields: &mut Vec<(ColumnInfo, PValue)>,
        labels: &mut Vec<String>,
        required: &mut Vec<bool>,
        masks: &mut Vec<String>,
        computed: &mut Vec<Option<String>>,
    ) -> Option<FormSpec> {
        let spec = self.cached_form(table)?;
        // Column index once (was O(F*C) position() scans per record).
        let index: HashMap<&str, usize> = fields
            .iter()
            .enumerate()
            .map(|(i, (c, _))| (c.name.as_str(), i))
            .collect();
        let mut ordered = Vec::new();
        let mut new_labels = Vec::new();
        let mut new_required = Vec::new();
        let mut new_masks = Vec::new();
        let mut new_computed = Vec::new();
        for f in spec.fields.iter().filter(|f| f.include) {
            if let Some(&idx) = index.get(f.column.as_str()) {
                ordered.push(fields[idx].clone());
                new_labels.push(f.label.clone());
                new_required.push(f.required);
                new_masks.push(f.mask.clone());
                new_computed.push(None);
            } else if !f.computed.is_empty() {
                // A derived column: not in the table, shown read-only.
                let alias = if f.column.is_empty() {
                    f.label.clone()
                } else {
                    f.column.clone()
                };
                ordered.push((
                    ColumnInfo {
                        name: alias,
                        decl_type: String::new(),
                        notnull: false,
                        pk: false,
                        dflt_value: None,
                    },
                    PValue::Null,
                ));
                new_labels.push(f.label.clone());
                new_required.push(false);
                new_masks.push(String::new());
                new_computed.push(Some(f.computed.clone()));
            }
        }
        if !ordered.is_empty() {
            *fields = ordered;
            *labels = new_labels;
            *required = new_required;
            *masks = new_masks;
            *computed = new_computed;
        }
        spec.painted().then_some(spec)
    }

    /// Evaluate every computed field for `rowid` in one query; on error
    /// the cells show `err: …` rather than silently going blank.
    fn eval_computed(
        &self,
        table: &str,
        rowid: i64,
        computed: &[Option<String>],
        fields: &mut [(ColumnInfo, PValue)],
    ) {
        let idxs: Vec<usize> = computed
            .iter()
            .enumerate()
            .filter_map(|(i, c)| c.as_ref().map(|_| i))
            .collect();
        if idxs.is_empty() || rowid == 0 {
            return;
        }
        let select: Vec<String> = idxs
            .iter()
            .map(|&i| format!("({})", computed[i].as_deref().unwrap_or("NULL")))
            .collect();
        let sql = format!(
            "SELECT {} FROM {} WHERE rowid = {rowid}",
            select.join(", "),
            Self::quote_ident(table)
        );
        match self.db.query(&sql) {
            Ok(q) => {
                if let Some(row) = q.rows.first() {
                    for (j, &i) in idxs.iter().enumerate() {
                        fields[i].1 = row.get(j).cloned().unwrap_or(PValue::Null);
                    }
                }
            }
            Err(e) => {
                for &i in &idxs {
                    fields[i].1 = PValue::Text(format!("err: {e}"));
                }
            }
        }
    }

    /// 'a' in BROWSE: a blank record form; save INSERTs (crafted forms
    /// and required validation apply exactly as for EDIT).
    fn open_insert(&mut self) {
        let Some(Grid {
            source: GridSource::Table { name, editable },
            ..
        }) = &self.grid
        else {
            return self.say("insert needs a table BROWSE (query results are read-only)");
        };
        if !editable {
            return self.say("this table has no rowid; cannot insert here");
        }
        let name = name.clone();
        let cols = match self.cached_columns(&name) {
            Ok(c) => c,
            Err(e) => return self.err(e),
        };
        let mut fields: Vec<(ColumnInfo, PValue)> =
            cols.into_iter().map(|c| (c, PValue::Null)).collect();
        let mut labels: Vec<String> = fields.iter().map(|(c, _)| c.name.clone()).collect();
        let mut required: Vec<bool> = vec![false; fields.len()];
        let mut masks: Vec<String> = vec![String::new(); fields.len()];
        let mut computed: Vec<Option<String>> = vec![None; fields.len()];
        let painted = self.apply_crafted_form(
            &name,
            &mut fields,
            &mut labels,
            &mut required,
            &mut masks,
            &mut computed,
        );
        let pickers = self.build_pickers(&name, &fields);
        let n = fields.len();
        self.overlay = Overlay::Edit(EditState {
            table: name,
            inserting: true,
            painted,
            links: Vec::new(), // a NEW record has no key to relate on yet
            row_abs: 0,
            rowid: 0,
            fields,
            labels,
            required,
            masks,
            computed,
            inputs: vec![None; n],
            cursor: 0,
            editing: None,
            pickers,
            picker: None,
        });
    }

    /// 'x' in BROWSE: armed double-press delete of the current row.
    fn delete_row(&mut self) {
        let Some(Grid {
            source: GridSource::Table { name, editable },
            cur_row,
            cache_start,
            rowids,
            ..
        }) = &self.grid
        else {
            return self.say("delete needs a table BROWSE");
        };
        if !editable {
            return self.say("this table has no rowid; cannot delete here");
        }
        let idx = (cur_row - cache_start) as usize;
        let Some(rowid) = rowids.as_ref().and_then(|r| r.get(idx)).copied() else {
            return;
        };
        let table = name.clone();
        if self.pending_delete == Some((table.clone(), rowid)) {
            self.pending_delete = None;
            match self.db.delete_row(&table, rowid) {
                Ok(()) => {
                    self.pane_cache.clear(); // child counts changed
                    self.invalidate_health();
                    self.refresh_grid_keep_position();
                    self.say("row deleted");
                }
                Err(e) => self.err(e),
            }
        } else {
            self.pending_delete = Some((table, rowid));
            self.err(format!("press x again to DELETE rowid {rowid}"));
        }
    }

    /// Refresh the current table grid without losing the cursor.
    /// Fresh total + current window via open_window (one trip),
    /// keeping columns/widths/editable. grid_jump then re-pages only
    /// if the clamped cursor left the fetched window.
    fn refresh_grid_keep_position(&mut self) {
        let (name, row, col, want_start, limit) = match &self.grid {
            Some(g) => match &g.source {
                GridSource::Table { name, .. } => {
                    let limit = (g.cache.len() as i64).max(1);
                    (name.clone(), g.cur_row, g.cur_col, g.cache_start, limit)
                }
                _ => return,
            },
            None => return,
        };
        let start = std::time::Instant::now();
        match self.db.open_window(&name, want_start, limit) {
            Ok((page, total)) => {
                if let Some(g) = &mut self.grid {
                    g.total = total;
                    g.cache = page.rows;
                    g.rowids = page.rowids;
                    g.cache_start = want_start;
                    g.cur_col = col.min(g.columns.len().saturating_sub(1));
                }
                self.last_ms = Some(start.elapsed().as_secs_f64() * 1000.0);
            }
            Err(e) => return self.err(e),
        }
        self.grid_jump(row);
    }

    /// `find <text>` / 'n': scan forward from the cursor for a row with
    /// any cell containing the needle (case-insensitive). Client-side
    /// scan in pages; capped so a miss on a huge table stays bounded.
    /// (Pushed to SQL per-cell would shift OFFSET windows — positions
    /// must come from the same scan order the grid pages in — so the
    /// win here is zero-alloc matching instead of render()+lowercase
    /// temporaries per cell.)
    fn find(&mut self, needle: &str) {
        const SCAN_CAP: i64 = 100_000;
        let needle_lc = needle.to_ascii_lowercase();
        let needle_has_alpha = needle_lc.bytes().any(|b| b.is_ascii_alphabetic());
        let matches = |row: &[PValue]| {
            row.iter()
                .any(|v| v.contains_ci(&needle_lc, needle_has_alpha))
        };
        let Some(g) = &self.grid else {
            return self.say("find works in a grid");
        };
        let (start, total) = (g.cur_row + 1, g.total);
        let table = match &g.source {
            GridSource::Table { name, .. } => Some(name.clone()),
            GridSource::Query { .. } | GridSource::Detail { .. } => None,
        };
        self.last_find = Some(needle.to_owned());
        let Some(table) = table else {
            // Query results live in memory: synchronous scan, unchanged.
            let g = self.grid.as_ref().unwrap();
            let hit = (start..total).find(|&abs| g.row(abs).is_some_and(|row| matches(row)));
            return match hit {
                Some(abs) => {
                    self.grid_jump(abs);
                    self.focus = Focus::Grid;
                    self.say(format!("found at row {}", abs + 1));
                }
                None => self.say(format!(
                    "{needle:?} not found below (g for top, n to retry)"
                )),
            };
        };
        // Table scan, async: submit the first window; each response
        // scans its rows and chains the next window until hit or cap.
        // The UI stays live throughout (keys keep flowing between trips).
        let end = total.min(start + SCAN_CAP);
        self.find_seq += 1;
        let seq = self.find_seq;
        let sseq = self.status_seq;
        let limit = Self::FIND_PAGE.min(end - start).max(0);
        let op = PendingOp::Find {
            table: table.clone(),
            needle: needle.to_owned(),
            needle_lc,
            needle_has_alpha,
            offset: start,
            end,
            seq,
            sseq,
        };
        match self.db.submit(Box::new(move |db| {
            DbResponse::Page(db.page(&table, start, limit))
        })) {
            Some(tag) => {
                self.pending.insert(tag, op);
            }
            None => self.err("database worker is gone"),
        }
    }

    fn find_next(&mut self) {
        if let Some(n) = self.last_find.clone() {
            self.find(&n);
        } else {
            self.say("no previous find (use: find <text>)");
        }
    }

    /// Tab at the prompt: complete the last token against table names
    /// and prompt commands. Borrows candidates (no per-Tab Vec<String>).
    fn prompt_complete(&mut self) {
        let (head, token) = match self.prompt.input.rfind(char::is_whitespace) {
            Some(i) => (
                self.prompt.input[..=i].to_owned(),
                self.prompt.input[i + 1..].to_owned(),
            ),
            None => (String::new(), self.prompt.input.clone()),
        };
        if token.is_empty() {
            return;
        }
        const COMMANDS: &[&str] = &[
            "select",
            "help",
            "tables",
            "health",
            "qbe",
            "report",
            "labels",
            "quit",
            "form",
            "apps",
            "app",
            "run",
            "find",
            "import",
            "export",
            "advise",
            "script",
            "scripts",
            "edit",
            "set theme",
        ];
        let matches: Vec<&str> = self
            .tables
            .iter()
            .map(|t| t.name.as_str())
            .chain(COMMANDS.iter().copied())
            .filter(|c| c.starts_with(token.as_str()) && *c != token.as_str())
            .collect();
        match matches.len() {
            0 => self.say(format!("no completion for {token:?}")),
            1 => {
                self.prompt.input = format!("{head}{}", matches[0]);
                self.prompt.cursor = self.prompt.input.len();
            }
            _ => {
                let list: Vec<&str> = matches.iter().take(6).copied().collect();
                self.say(list.join(" · "));
            }
        }
    }

    fn edit_save(&mut self) {
        if self.commit_edit() {
            self.overlay = Overlay::None;
        }
    }

    /// Quietly true when every required field of the open EDIT has a
    /// value (the loud version lives in commit_edit).
    fn edit_required_ok(&self) -> bool {
        let Overlay::Edit(ed) = &self.overlay else {
            return false;
        };
        ed.required.iter().enumerate().all(|(i, req)| {
            !req || match &ed.inputs[i] {
                Some(text) => PValue::parse(text, &ed.fields[i].0.decl_type) != PValue::Null,
                None => ed.fields[i].1 != PValue::Null,
            }
        })
    }

    /// An open field-editing buffer counts as an edit: fold it into
    /// inputs so save/paging never silently drop typed text.
    fn fold_editing_buffer(&mut self) {
        if let Overlay::Edit(ed) = &mut self.overlay {
            if let Some(buf) = ed.editing.take() {
                ed.inputs[ed.cursor] = Some(buf);
            }
        }
    }

    /// Validate + write the current EDIT record. Returns true when the
    /// record is clean afterwards (saved, or nothing to save). Does NOT
    /// close the overlay — F10 closes, paging keeps flying.
    fn commit_edit(&mut self) -> bool {
        self.commit_edit_inner(false)
    }

    fn commit_edit_inner(&mut self, skip_required_check: bool) -> bool {
        self.fold_editing_buffer();
        // Required validation (crafted forms): the FINAL value of every
        // required field must be non-NULL, edited or not.
        if !skip_required_check {
            if let Overlay::Edit(ed) = &self.overlay {
                for (i, req) in ed.required.iter().enumerate() {
                    if !req {
                        continue;
                    }
                    let is_null = match &ed.inputs[i] {
                        Some(text) => {
                            PValue::parse(text, &ed.fields[i].0.decl_type) == PValue::Null
                        }
                        None => ed.fields[i].1 == PValue::Null,
                    };
                    if is_null {
                        let label = ed.labels[i].clone();
                        self.err(format!("{label:?} is required"));
                        return false;
                    }
                }
            }
        }
        // PICTURE masks: an entered value must fit its mask.
        if let Overlay::Edit(ed) = &self.overlay {
            for (i, mask) in ed.masks.iter().enumerate() {
                if mask.is_empty() {
                    continue;
                }
                if let Some(text) = &ed.inputs[i] {
                    if !mask_ok(mask, text) {
                        let label = ed.labels[i].clone();
                        self.err(format!("{label:?} must match {mask}"));
                        return false;
                    }
                }
            }
        }
        // Form lifecycle (rule 4): OnValidate may block or set fields.
        // The Enter path is quiet; F10/paging surface the reason.
        if !self.run_validate_script(skip_required_check) {
            return false;
        }
        let payload = match &self.overlay {
            Overlay::Edit(ed) if !ed.dirty() && !ed.inserting => None,
            Overlay::Edit(ed) => {
                let changes: Vec<(String, PValue)> = ed
                    .fields
                    .iter()
                    .enumerate()
                    // Computed columns are shown, never written.
                    .filter(|(i, _)| !matches!(ed.computed.get(*i), Some(Some(_))))
                    .filter_map(|(i, (col, _))| {
                        ed.inputs[i]
                            .as_ref()
                            .map(|text| (col.name.clone(), PValue::parse(text, &col.decl_type)))
                    })
                    .collect();
                Some((ed.table.clone(), ed.rowid, changes, ed.inserting))
            }
            _ => return false,
        };
        let Some((table, rowid, changes, inserting)) = payload else {
            self.say("no changes");
            return true;
        };
        let n = changes.len();
        let result = if inserting {
            self.db
                .insert_row(&table, &changes)
                .map(|rowid| (rowid, format!("inserted rowid {rowid}")))
        } else {
            self.db
                .update_row(&table, rowid, &changes)
                .map(|()| (rowid, format!("saved {n} field(s)")))
        };
        match result {
            Ok((new_rowid, msg)) => {
                // The write may have changed child counts/previews and health.
                self.pane_cache.clear();
                self.invalidate_health();
                if inserting {
                    // Total changed: full refresh, then flip the open
                    // form onto the newly inserted record so further
                    // Enters UPDATE it instead of inserting twins.
                    // Synchronous tail fetch (NOT parked): a second
                    // save before arrival would otherwise insert twins
                    // — the flip must land on the new record NOW.
                    self.refresh_grid_keep_position();
                    let last = self.grid.as_ref().map(|g| g.total - 1).unwrap_or(0);
                    let cursor = match &self.overlay {
                        Overlay::Edit(ed) => ed.cursor,
                        _ => 0,
                    };
                    self.flip_to_tail(last.max(0), new_rowid);
                    if let Overlay::Edit(ed) = &mut self.overlay {
                        ed.cursor = cursor.min(ed.fields.len().saturating_sub(1));
                    }
                } else {
                    // Re-fetch the live window async (stale rows stay
                    // until swap); a parked edit builds on arrival.
                    self.refresh_window();
                    // The record on screen is clean now — fold the
                    // committed inputs into the snapshot the form
                    // displays, or saved values would revert to the
                    // stale (often NULL) originals on screen.
                    if let Overlay::Edit(ed) = &mut self.overlay {
                        for (i, input) in ed.inputs.iter_mut().enumerate() {
                            if let Some(text) = input.take() {
                                ed.fields[i].1 = PValue::parse(&text, &ed.fields[i].0.decl_type);
                            }
                        }
                    }
                }
                // OnSave runs after the row is committed (side effects,
                // messages); its field edits are ignored — the record is
                // already written.
                match self.run_save_script() {
                    Ok(notes) if !notes.is_empty() => {
                        self.say(format!("{msg} · {}", notes.join(" · ")))
                    }
                    Ok(_) => self.say(msg),
                    Err(e) => self.err(format!("{msg} · OnSave: {e}")),
                }
                true
            }
            Err(e) => {
                self.err(e);
                false
            }
        }
    }

    /// Final field values of the open EDIT as `(column, value)`.
    fn edit_values(&self) -> Option<EditValues> {
        let Overlay::Edit(ed) = &self.overlay else {
            return None;
        };
        let values = ed
            .fields
            .iter()
            .enumerate()
            .map(|(i, (c, _))| {
                let v = match &ed.inputs[i] {
                    Some(t) => PValue::parse(t, &c.decl_type),
                    None => ed.fields[i].1.clone(),
                };
                (c.name.clone(), v)
            })
            .collect();
        let field = ed.fields.get(ed.cursor).map(|(c, _)| c.name.clone());
        Some((ed.table.clone(), ed.inserting, field, values))
    }

    /// Run `OnValidate` (if bound). Returns false to block the save.
    fn run_validate_script(&mut self, quiet: bool) -> bool {
        self.run_edit_script("OnValidate", quiet, true)
    }

    /// Run a form/field lifecycle script and fold any fields it set back
    /// in as ordinary edits. `blocking` scripts return false on `error`.
    fn run_edit_script(&mut self, event: &str, quiet: bool, blocking: bool) -> bool {
        let Some((table, inserting, field, mut values)) = self.edit_values() else {
            return true;
        };
        let Some(src) = crate::script::get_script(self.db.link(), &table, event) else {
            return true;
        };
        let outcome = match crate::script::run_form_event(
            self.db.link(),
            &src,
            &mut values,
            field.as_deref(),
            inserting,
        ) {
            Ok(o) => o,
            Err(e) => {
                if !quiet {
                    self.err(format!("{event}: {e}"));
                }
                return !blocking;
            }
        };
        if !quiet {
            for m in &outcome.messages {
                self.say(m.clone());
            }
            if let Some(e) = &outcome.error {
                self.err(e.clone());
            }
        }
        if blocking && outcome.error.is_some() {
            return false;
        }
        // Fold script-set values back in as if the user had typed them.
        if let Overlay::Edit(ed) = &mut self.overlay {
            for (name, v) in &values {
                if let Some(i) = ed.fields.iter().position(|(c, _)| &c.name == name) {
                    let current = match &ed.inputs[i] {
                        Some(t) => PValue::parse(t, &ed.fields[i].0.decl_type),
                        None => ed.fields[i].1.clone(),
                    };
                    if &current != v {
                        ed.inputs[i] = Some(pvalue_to_input(v));
                    }
                }
            }
        }
        true
    }

    /// Run `OnSave` (if bound) after a successful write; returns notes.
    fn run_save_script(&mut self) -> Result<Vec<String>, String> {
        let Some((table, inserting, field, mut values)) = self.edit_values() else {
            return Ok(Vec::new());
        };
        let Some(src) = crate::script::get_script(self.db.link(), &table, "OnSave") else {
            return Ok(Vec::new());
        };
        let outcome = crate::script::run_form_event(
            self.db.link(),
            &src,
            &mut values,
            field.as_deref(),
            inserting,
        )?;
        if let Some(e) = outcome.error {
            return Err(e);
        }
        Ok(outcome.messages)
    }

    fn prompt_history(&mut self, d: i64) {
        let len = self.prompt.history.len();
        if len == 0 {
            return;
        }
        let pos = match (self.prompt.hist_pos, d) {
            (None, -1) => Some(len - 1),
            (None, _) => None,
            (Some(0), -1) => Some(0),
            (Some(p), -1) => Some(p - 1),
            (Some(p), _) if p + 1 >= len => None,
            (Some(p), _) => Some(p + 1),
        };
        self.prompt.hist_pos = pos;
        self.prompt.input = pos
            .map(|p| self.prompt.history[p].clone())
            .unwrap_or_default();
        self.prompt.cursor = self.prompt.input.len();
    }

    fn prompt_run(&mut self) {
        let line = self.prompt.input.trim().to_owned();
        if line.is_empty() {
            return;
        }
        // Cap history: an unbounded Vec<String> grows for the whole
        // session (every Enter appends). hist_pos resets on run, so
        // shifting is safe here.
        const HIST_CAP: usize = 512;
        self.prompt.history.push(line.clone());
        if self.prompt.history.len() > HIST_CAP {
            self.prompt.history.remove(0);
        }
        self.prompt.hist_pos = None;
        self.prompt.input.clear();
        self.prompt.cursor = 0;

        // App commands first, SQL otherwise.
        if let Some(rest) = line.strip_prefix("set theme ") {
            return match Theme::by_name(rest.trim()) {
                Some(t) => {
                    self.theme = t;
                    store::pref_set(self.db.link(), "theme", t.name);
                    self.say(format!("theme: {} (remembered)", t.name));
                }
                None => self.err(format!(
                    "unknown theme {:?}; themes: green, amber, paper, blue",
                    rest.trim()
                )),
            };
        }
        if let Some(rest) = line.strip_prefix("set shimmer ") {
            return match rest.trim().to_ascii_lowercase().as_str() {
                "on" => {
                    self.shimmer = true;
                    store::pref_set(self.db.link(), "shimmer", "on");
                    self.say("shimmer: on (remembered)");
                }
                "off" => {
                    self.shimmer = false;
                    store::pref_set(self.db.link(), "shimmer", "off");
                    self.say("shimmer: off (remembered)");
                }
                other => self.err(format!("set shimmer on|off (got {other:?})")),
            };
        }
        if line == "help" {
            return self.open_help();
        }
        // dBASE ended sessions with QUIT; honor it (and friends).
        if ["quit", "exit", "q"]
            .iter()
            .any(|w| line.eq_ignore_ascii_case(w))
        {
            return self.apply(Command::Quit);
        }
        if line == "health" {
            return self.open_health();
        }
        if let Some(rest) = line.strip_prefix("edit ") {
            let mut it = rest.splitn(2, char::is_whitespace);
            let table = it.next().unwrap_or("").trim();
            let event = it.next().unwrap_or("").trim();
            if table.is_empty() || event.is_empty() {
                return self.err("usage: edit <table> <event>  (OnValidate, OnSave, OnChange)");
            }
            return self.apply(Command::OpenFormScript {
                table: table.to_owned(),
                event: event.to_owned(),
            });
        }
        if let Some(rest) = line.strip_prefix("script ") {
            return self.set_form_script(rest);
        }
        if line == "scripts" || line.starts_with("scripts ") {
            let t = line["scripts".len()..].trim();
            return self.list_scripts(t);
        }
        if line == "advise" || line == "advisor" {
            return match crate::advisor::advise(self.db.link()) {
                Ok(lines) => {
                    self.overlay = Overlay::Pager(PagerState {
                        title: " ADVISOR ".into(),
                        lines,
                        offset: 0,
                        file_stem: "advisor".into(),
                    });
                }
                Err(e) => self.err(e),
            };
        }
        if let Some(rest) = line.strip_prefix("qbe") {
            let t = rest.trim();
            return self.open_qbe((!t.is_empty()).then(|| t.to_owned()));
        }
        if let Some(rest) = line.strip_prefix("report") {
            let t = rest.trim();
            return self.open_report((!t.is_empty()).then(|| t.to_owned()));
        }
        if let Some(rest) = line.strip_prefix("labels") {
            let t = rest.trim();
            return self.open_labels((!t.is_empty()).then(|| t.to_owned()));
        }
        if let Some(rest) = line.strip_prefix("run ") {
            let name = rest.trim();
            return match QbeSpec::saved_sql(self.db.link(), name) {
                Some(sql) => self.run_select(&sql),
                None => self.err(format!("no saved query named {name:?}")),
            };
        }
        {
            let toks: Vec<&str> = line.split_whitespace().collect();
            if toks
                .first()
                .is_some_and(|t| t.eq_ignore_ascii_case("create"))
                && toks.len() <= 2
            {
                let second = toks.get(1).map(|t| t.to_ascii_lowercase());
                let sql_word = matches!(
                    second.as_deref(),
                    Some(
                        "table"
                            | "virtual"
                            | "view"
                            | "index"
                            | "trigger"
                            | "temp"
                            | "temporary"
                            | "unique"
                            | "if"
                    )
                );
                if !sql_word {
                    // `create` / `create gadgets` → the TABLE DESIGNER.
                    // Anything SQL-shaped falls through and executes.
                    return self.open_create(toks.get(1).map(|t| t.to_string()));
                }
            }
        }
        if let Some(rest) = line.strip_prefix("find ") {
            let needle = rest.trim().to_owned();
            if !needle.is_empty() {
                return self.find(&needle);
            }
        }
        if let Some(rest) = line.strip_prefix("form") {
            let t = rest.trim();
            return self.open_form((!t.is_empty()).then(|| t.to_owned()));
        }
        if let Some(rest) = line.strip_prefix("apps") {
            let t = rest.trim();
            return self.open_apps((!t.is_empty()).then(|| t.to_owned()));
        }
        if let Some(rest) = line.strip_prefix("app ") {
            let t = rest.trim();
            return self.open_app_menu((!t.is_empty()).then(|| t.to_owned()));
        }
        if line == "app" {
            return self.open_app_menu(None);
        }
        if line == "tables" {
            self.reload_tables();
            self.focus = Focus::Sidebar;
            return;
        }
        if line.starts_with("import") {
            return self.handle_import(&line);
        }
        if line.starts_with("export") {
            return self.handle_export(&line);
        }

        let head = line
            .split_whitespace()
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();
        if matches!(
            head.as_str(),
            "select" | "with" | "pragma" | "explain" | "values"
        ) {
            self.run_select(&line);
        } else if self.readonly {
            self.err("read-only mode: only SELECT is allowed");
        } else {
            match self.db.execute(&line) {
                Ok((n, elapsed)) => {
                    self.last_ms = Some(elapsed.as_secs_f64() * 1000.0);
                    self.reload_tables();
                    self.refresh_health();
                    self.say(match n {
                        -1 => "ok (batch)".to_owned(),
                        n => format!("ok, {n} row(s) affected"),
                    });
                }
                Err(e) => self.err(e),
            }
        }
    }

    fn handle_import(&mut self, line: &str) {
        if self.readonly {
            return self.err("read-only mode: import is disabled");
        }
        let rest = strip_csv_keyword(line["import".len()..].trim());
        // table and path are last two whitespace-separated tokens; path may be quoted
        let mut parts = rest.rsplitn(2, char::is_whitespace);
        let raw_path = parts.next().unwrap_or("").trim();
        let raw_table = parts.next().unwrap_or("").trim();
        let path = strip_quotes(raw_path);
        let table = strip_quotes(raw_table);
        if table.is_empty() || path.is_empty() {
            self.err("usage: import <table> <path>  — e.g. import customers ./data.csv");
            return;
        }
        match crate::csv_io::import_csv(self.db.link(), table, path) {
            Ok(msg) => {
                self.reload_tables();
                self.refresh_health();
                self.say(msg);
            }
            Err(e) => self.err(e),
        }
    }

    fn handle_export(&mut self, line: &str) {
        let rest = strip_csv_keyword(line["export".len()..].trim());
        let idx = rest.rfind(char::is_whitespace);
        let (raw_source, raw_path) = match idx {
            Some(i) => (rest[..i].trim(), rest[i + 1..].trim()),
            None => {
                self.err(
                    "usage: export <table|SELECT> <path>  — e.g. export customers ./out.csv or export \"select * from customers\" ./out.csv",
                );
                return;
            }
        };
        let source = strip_quotes(raw_source);
        let path = strip_quotes(raw_path);
        if source.is_empty() || path.is_empty() {
            self.err("usage: export <table|SELECT> <path>");
            return;
        }
        match crate::csv_io::export_csv(self.db.link(), source, path) {
            Ok(msg) => self.say(msg),
            Err(e) => self.err(e),
        }
    }

    /// `script <table> <event> <lua>` — bind a form lifecycle script.
    /// An empty lua body clears the binding.
    fn set_form_script(&mut self, rest: &str) {
        if self.readonly {
            return self.err("read-only mode: cannot bind scripts");
        }
        let mut parts = rest.splitn(3, char::is_whitespace);
        let table = parts.next().unwrap_or("").trim();
        let event = parts.next().unwrap_or("").trim();
        let source = parts.next().unwrap_or("").trim();
        if table.is_empty() || event.is_empty() {
            return self
                .err("usage: script <table> <OnValidate|OnSave|OnChange> <lua>  (no lua clears)");
        }
        let Some(ev) = crate::script::normalize_event(event) else {
            return self.err("events: OnValidate, OnSave, OnChange");
        };
        if source.is_empty() {
            return match crate::script::clear_script(self.db.link(), table, ev) {
                Ok(()) => self.say(format!("cleared {table} {ev}")),
                Err(e) => self.err(e),
            };
        }
        match crate::script::set_script(self.db.link(), table, ev, source) {
            Ok(()) => self.say(format!(
                "saved {table} {ev} ({} chars) — see: scripts {table}",
                source.chars().count()
            )),
            Err(e) => self.err(e),
        }
    }

    /// `scripts [table]` — list bound lifecycle scripts in a pager.
    fn list_scripts(&mut self, table: &str) {
        let all = crate::script::all_scripts(self.db.link());
        let mut lines = vec![
            "LIFECYCLE SCRIPTS".to_owned(),
            "bind:  script <table> <event> <lua>".to_owned(),
            format!("events: {}", crate::script::EVENTS.join(" · ")),
            String::new(),
        ];
        let filtered: Vec<_> = all
            .into_iter()
            .filter(|(t, _, _)| table.is_empty() || t.eq_ignore_ascii_case(table))
            .collect();
        if filtered.is_empty() {
            lines.push(if table.is_empty() {
                "no scripts bound in this database".to_owned()
            } else {
                format!("no scripts bound for {table}")
            });
        }
        for (t, ev, src) in filtered {
            lines.push(format!("{t}  {ev}   (edit {t} {ev})"));
            for l in src.lines() {
                lines.push(format!("  {l}"));
            }
            lines.push(String::new());
        }
        // Menu-item scripts live on `_phosphor_items`, not the lifecycle
        // table; show them too when unfiltered.
        if table.is_empty() {
            if let Ok(q) = self.db.query(
                "SELECT a.name, i.label, i.action_ref \
                 FROM _phosphor_items i JOIN _phosphor_apps a ON a.id = i.app_id \
                 WHERE i.action_kind = 'script' ORDER BY a.name, i.seq, i.id",
            ) {
                let items: Vec<(String, String, String)> = q
                    .rows
                    .iter()
                    .filter_map(|r| match (r.first(), r.get(1), r.get(2)) {
                        (Some(PValue::Text(a)), Some(PValue::Text(l)), Some(PValue::Text(s))) => {
                            Some((a.clone(), l.clone(), s.clone()))
                        }
                        _ => None,
                    })
                    .collect();
                if !items.is_empty() {
                    lines.push("MENU-ITEM SCRIPTS (A, then E on the item)".to_owned());
                    lines.push(String::new());
                    for (app, label, src) in items {
                        lines.push(format!("{app} · {label}"));
                        for l in src.lines() {
                            lines.push(format!("  {l}"));
                        }
                        lines.push(String::new());
                    }
                }
            }
        }
        self.overlay = Overlay::Pager(PagerState {
            title: if table.is_empty() {
                " SCRIPTS ".into()
            } else {
                format!(" SCRIPTS · {table} ")
            },
            lines,
            offset: 0,
            file_stem: "scripts".into(),
        });
    }

    // ── the multi-line script editor ─────────────────────────────────

    fn open_form_script(&mut self, table: String, event: String) {
        if self.readonly {
            return self.err("read-only mode: cannot edit scripts");
        }
        let Some(ev) = crate::script::normalize_event(&event) else {
            return self.err("events: OnValidate, OnSave, OnChange");
        };
        let src = crate::script::get_script(self.db.link(), &table, ev).unwrap_or_default();
        self.script_return_app = None;
        self.overlay = Overlay::ScriptEditor(ScriptState::new(
            ScriptTarget::Form {
                table,
                event: ev.to_owned(),
            },
            &src,
        ));
    }

    /// `E` in the Applications Generator: edit the selected item's Lua
    /// source full-screen. Only meaningful for `script` items.
    fn open_selected_item_script(&mut self) {
        if self.readonly {
            return self.err("read-only mode: cannot edit scripts");
        }
        let (app, item) = match &self.overlay {
            Overlay::Apps(st) => (st.app.clone(), st.items.get(st.cursor).cloned()),
            _ => return,
        };
        let Some(item) = item else {
            return self.say("no menu item selected");
        };
        if item.kind != ActionKind::Script {
            return self.say("E edits a `script` item's Lua (c cycles the kind)");
        }
        self.script_return_app = Some(app.clone());
        self.overlay = Overlay::ScriptEditor(ScriptState::new(
            ScriptTarget::MenuItem {
                app,
                item_id: item.id,
                label: item.label.clone(),
            },
            &item.action_ref,
        ));
    }

    /// Leave the editor; when it was opened from the Applications
    /// Generator, go back there instead of the bare browser.
    fn close_script_editor(&mut self) {
        match self.script_return_app.take() {
            Some(app) => self.open_apps(Some(app)),
            None => self.overlay = Overlay::None,
        }
    }

    fn script_char(&mut self, c: char) {
        if let Overlay::ScriptEditor(st) = &mut self.overlay {
            let b = byte_at(&st.lines[st.row], st.col);
            st.lines[st.row].insert(b, c);
            st.col += 1;
            st.dirty = true;
        }
    }

    fn script_newline(&mut self) {
        if let Overlay::ScriptEditor(st) = &mut self.overlay {
            let b = byte_at(&st.lines[st.row], st.col);
            let rest = st.lines[st.row].split_off(b);
            st.lines.insert(st.row + 1, rest);
            st.row += 1;
            st.col = 0;
            st.dirty = true;
        }
    }

    fn script_backspace(&mut self) {
        if let Overlay::ScriptEditor(st) = &mut self.overlay {
            if st.col > 0 {
                let b = byte_at(&st.lines[st.row], st.col);
                let prev = st.lines[st.row][..b]
                    .chars()
                    .next_back()
                    .map_or(1, |c| c.len_utf8());
                st.lines[st.row].replace_range(b - prev..b, "");
                st.col -= 1;
            } else if st.row > 0 {
                let joined = st.lines.remove(st.row);
                st.row -= 1;
                st.col = st.lines[st.row].chars().count();
                st.lines[st.row].push_str(&joined);
            }
            st.dirty = true;
        }
    }

    fn script_move(&mut self, dl: i64, dc: i64) {
        if let Overlay::ScriptEditor(st) = &mut self.overlay {
            if dc != 0 {
                let len = st.line_len() as i64;
                st.col = (st.col as i64 + dc).clamp(0, len) as usize;
            }
            if dl != 0 {
                let n = st.lines.len() as i64;
                st.row = (st.row as i64 + dl).clamp(0, n - 1) as usize;
                st.col = st.col.min(st.line_len());
            }
        }
    }

    fn script_line_edge(&mut self, end: bool) {
        if let Overlay::ScriptEditor(st) = &mut self.overlay {
            st.col = if end { st.line_len() } else { 0 };
        }
    }

    fn script_save(&mut self) {
        let Overlay::ScriptEditor(st) = &self.overlay else {
            return;
        };
        let text = st.text();
        let (result, note): (crate::db::DbResult<()>, String) = match &st.target {
            ScriptTarget::Form { table, event } => {
                let r = if text.trim().is_empty() {
                    crate::script::clear_script(self.db.link(), table, event)
                } else {
                    crate::script::set_script(self.db.link(), table, event, &text)
                };
                (r, format!("saved {table} {event}"))
            }
            ScriptTarget::MenuItem {
                app,
                item_id,
                label,
            } => (
                crate::appsgen::set_item_ref(self.db.link(), *item_id, &text),
                format!("saved {app} · {label}"),
            ),
        };
        match result {
            Ok(()) => {
                self.say(note);
                self.close_script_editor();
            }
            Err(e) => self.err(e),
        }
    }
}

/// An EDIT form's final values for a lifecycle script:
/// (table, inserting, current field, column/value pairs).
type EditValues = (String, bool, Option<String>, Vec<(String, PValue)>);

/// Drop an optional standalone `csv` token: `import csv t path` and
/// `import t path` both work, but a table actually named `csvtest` is
/// not mistaken for the keyword (found on film — the demo caught it).
fn strip_csv_keyword(s: &str) -> &str {
    let b = s.as_bytes();
    if b.len() >= 3 && b[..3].eq_ignore_ascii_case(b"csv") {
        let after = &s[3..];
        if after.starts_with(char::is_whitespace) {
            return after.trim_start();
        }
    }
    s
}

/// The editable text for a value a script set: raw text is preserved
/// (unlike `render`, which folds newlines), everything else renders.
fn pvalue_to_input(v: &PValue) -> String {
    match v {
        PValue::Text(t) => t.clone(),
        PValue::Null => String::new(),
        other => other.render(),
    }
}

/// Strip one matching pair of surrounding quotes. Only the SAME quote
/// character is removed from both ends — so `"select 'Ada'"` keeps its
/// inner single quotes (a `trim_matches` over both kinds ate the closing
/// `'`).
fn strip_quotes(s: &str) -> &str {
    let b = s.as_bytes();
    if b.len() >= 2 && (b[0] == b'"' || b[0] == b'\'') && b[b.len() - 1] == b[0] {
        &s[1..s.len() - 1]
    } else {
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::EmbeddedDb;

    fn app() -> App {
        let (db, _) = EmbeddedDb::open(":memory:").unwrap();
        db.execute(
            "CREATE TABLE t(a INTEGER PRIMARY KEY, b TEXT);
             INSERT INTO t(b)
               WITH RECURSIVE c(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM c WHERE x < 500)
               SELECT 'row' || x FROM c;",
        )
        .unwrap();
        App::new(Box::new(db), None)
    }

    #[test]
    fn open_browse_navigate_virtualized() {
        let mut a = app();
        a.apply(Command::OpenSelected);
        a.sync();
        assert_eq!(a.focus, Focus::Grid);
        let g = a.grid.as_ref().unwrap();
        assert_eq!(g.total, 500);
        a.apply(Command::GridBottom);
        a.sync();
        let g = a.grid.as_ref().unwrap();
        assert_eq!(g.cur_row, 499);
        assert!(g.row(499).is_some(), "cache must follow the cursor");
        a.apply(Command::GridTop);
        assert_eq!(a.grid.as_ref().unwrap().cur_row, 0);
    }

    #[test]
    fn prompt_select_becomes_query_grid() {
        let mut a = app();
        for c in "select count(*) as n from t".chars() {
            a.apply(Command::PromptChar(c));
        }
        a.apply(Command::PromptRun);
        a.sync();
        let g = a.grid.as_ref().unwrap();
        assert!(matches!(g.source, GridSource::Query { .. }));
        assert_eq!(g.row(0).unwrap()[0], PValue::Int(500));
    }

    #[test]
    fn edit_round_trip_through_the_bus() {
        let mut a = app();
        a.apply(Command::OpenSelected);
        a.sync();
        a.apply(Command::GridMove { dr: 0, dc: 1 });
        a.apply(Command::OpenEdit);
        assert!(matches!(a.overlay, Overlay::Edit(_)));
        a.apply(Command::EditMove(1)); // to column b
        a.apply(Command::EditBegin);
        if let Overlay::Edit(ed) = &mut a.overlay {
            ed.editing = Some(String::new());
        }
        for c in "edited!".chars() {
            a.apply(Command::EditChar(c));
        }
        a.apply(Command::EditCommitField);
        a.apply(Command::EditSave);
        a.sync(); // update path re-fetches the live window async
        assert!(matches!(a.overlay, Overlay::None));
        let g = a.grid.as_ref().unwrap();
        assert_eq!(g.row(0).unwrap()[1], PValue::Text("edited!".into()));
    }

    /// The windowed series query (open_health fast path) against a
    /// synthetic health base — no timeless extension needed.
    #[test]
    fn health_sparks_from_windowed_series_query() {
        let (db, _) = EmbeddedDb::open(":memory:").unwrap();
        db.execute(
            "CREATE TABLE m(name TEXT, value REAL, ts INTEGER);
             CREATE VIEW m_report AS
               SELECT 'c' AS \"check\", 'ok' AS status, 1.0 AS value, 'a' AS advice;
             INSERT INTO m VALUES
               ('cache_hits', 1.0, 1), ('cache_hits', 2.0, 2), ('cache_hits', 3.0, 3),
               ('other_metric', 10.0, 1), ('other_metric', 20.0, 2);",
        )
        .unwrap();
        let mut a = App::new(Box::new(db), None);
        a.apply(Command::OpenHealth);
        a.sync();
        let Overlay::Health(hv) = &a.overlay else {
            panic!("health console did not open: {:?}", a.status);
        };
        assert_eq!(hv.table, "m");
        assert_eq!(hv.report.len(), 1);
        assert_eq!(hv.sparks.len(), 2);
        let hits = hv
            .sparks
            .iter()
            .find(|(n, _, _)| n == "cache_hits")
            .unwrap();
        assert_eq!(hits.1, vec![1.0, 2.0, 3.0], "chronological after reverse");
    }

    /// The health dot is cached (TTL) and invalidated on demand:
    /// a dropped report view stays cached until invalidated.
    #[test]
    fn health_dot_cache_hit_and_invalidate() {
        let (db, _) = EmbeddedDb::open(":memory:").unwrap();
        db.execute(
            "CREATE VIEW m_report AS
               SELECT 'c' AS \"check\", 'ok' AS status, 1.0 AS value, 'a' AS advice;",
        )
        .unwrap();
        let mut a = App::new(Box::new(db), None);
        assert_eq!(a.health, Some("ok".into()));
        a.refresh_health();
        assert_eq!(a.health, Some("ok".into()));
        assert!(a.health_cache.is_some(), "dot cached after refresh");
        // The view goes away: cache still serves the dot.
        a.db.execute("DROP VIEW m_report").unwrap();
        a.refresh_health();
        assert_eq!(a.health, Some("ok".into()), "fresh cache wins");
        // Invalidate (what writes do): next refresh re-queries → None.
        a.invalidate_health();
        a.refresh_health();
        assert_eq!(a.health, None);
    }

    /// A health response arriving after the user moved on is dropped,
    /// never yanked over the new screen.
    #[test]
    fn stale_health_console_does_not_yank() {
        let (db, _) = EmbeddedDb::open(":memory:").unwrap();
        db.execute(
            "CREATE TABLE m(name TEXT, value REAL, ts INTEGER);
             CREATE VIEW m_report AS
               SELECT 'c' AS \"check\", 'ok' AS status, 1.0 AS value, 'a' AS advice;",
        )
        .unwrap();
        let mut a = App::new(Box::new(db), None);
        a.apply(Command::OpenHealth);
        // Navigate away BEFORE reconciling: QBE owns the screen now.
        a.apply(Command::OpenQbe(Some("m".into())));
        assert!(matches!(a.overlay, Overlay::Qbe(_)));
        a.sync();
        assert!(
            matches!(a.overlay, Overlay::Qbe(_)),
            "late health console must not clobber QBE"
        );
    }

    /// Full-stack phase 3, when the timeless extension is built next
    /// door: dbhealth vtab + samples + the console over the bus.
    #[test]
    fn health_console_over_timeless_extension() {
        let ext = "../timeless-libsql/target/release/libdbhealth_ext.so";
        if !std::path::Path::new(ext).exists() {
            eprintln!("skipping: {ext} not built");
            return;
        }
        std::env::set_var("PHOSPHOR_EXT", ext);
        let (db, warn) = EmbeddedDb::open(":memory:").unwrap();
        assert!(warn.is_none(), "extension failed to load: {warn:?}");
        db.execute("CREATE VIRTUAL TABLE dbhealth USING timeless_health")
            .unwrap();
        db.execute("INSERT INTO dbhealth(dbhealth) VALUES ('sample')")
            .unwrap();
        db.execute("INSERT INTO dbhealth(dbhealth) VALUES ('sample')")
            .unwrap();
        let mut a = App::new(Box::new(db), None);
        a.apply(Command::OpenHealth);
        a.sync();
        let Overlay::Health(hv) = &a.overlay else {
            panic!("health console did not open");
        };
        assert_eq!(hv.table, "dbhealth");
        assert!(hv.report.len() >= 7, "report rows: {}", hv.report.len());
        assert!(!hv.sparks.is_empty(), "no sparkline series");
        a.apply(Command::HealthSample);
        a.sync();
        assert!(matches!(a.overlay, Overlay::Health(_)));
        assert!(a.health.is_some(), "status dot missing after sample");
        a.apply(Command::Back);
        assert!(matches!(a.overlay, Overlay::None));
    }

    #[test]
    fn qbe_flow_through_the_bus() {
        let mut a = app();
        a.apply(Command::OpenQbe(Some("t".into())));
        assert!(matches!(a.overlay, Overlay::Qbe(_)));
        // Filter on column b: bare value → equality.
        a.apply(Command::DesignerMove(1));
        a.apply(Command::DesignerEditBegin);
        if let Overlay::Qbe(st) = &mut a.overlay {
            st.editing = Some(String::new());
        }
        for c in "row42".chars() {
            a.apply(Command::DesignerChar(c));
        }
        a.apply(Command::DesignerCommit);
        // Save under a name, then run.
        a.apply(Command::DesignerSave);
        if let Overlay::Qbe(st) = &mut a.overlay {
            st.editing = Some("just42".into());
        }
        a.apply(Command::DesignerCommit);
        a.apply(Command::DesignerRun);
        a.sync();
        let g = a.grid.as_ref().unwrap();
        assert_eq!(g.total, 1, "exactly row42 matches");
        assert_eq!(g.row(0).unwrap()[1], PValue::Text("row42".into()));
        // And the saved query runs by name from the prompt.
        for c in "run just42".chars() {
            a.apply(Command::PromptChar(c));
        }
        a.apply(Command::PromptRun);
        a.sync();
        assert_eq!(a.grid.as_ref().unwrap().total, 1);
    }

    /// #12: an OnValidate lifecycle script can block a save and rewrite
    /// fields before the write.
    #[test]
    fn form_validation_script_blocks_and_sets() {
        let (db, _) = EmbeddedDb::open(":memory:").unwrap();
        db.execute(
            "CREATE TABLE people(id INTEGER PRIMARY KEY, name TEXT);
             INSERT INTO people(name) VALUES ('ada');",
        )
        .unwrap();
        let mut a = App::new(Box::new(db), None);
        let bind = r#"script people OnValidate if record.name == "" or record.name == nil then error("need a name") else set("name", string.upper(record.name)) end"#;
        for c in bind.chars() {
            a.apply(Command::PromptChar(c));
        }
        a.apply(Command::PromptRun);
        a.sync();
        assert!(
            !a.status.as_ref().is_some_and(|(_, e)| *e),
            "bind: {:?}",
            a.status
        );

        a.apply(Command::SidebarSeek('p')); // people
        a.apply(Command::OpenSelected);
        a.sync();
        a.apply(Command::OpenEdit);
        a.apply(Command::EditMove(1)); // name
        a.apply(Command::EditBegin);
        if let Overlay::Edit(ed) = &mut a.overlay {
            ed.editing = Some(String::new());
        }
        a.apply(Command::EditCommitField); // Enter: quiet block, no toast
        assert!(
            !a.status.as_ref().is_some_and(|(_, e)| *e),
            "quiet path must not toast: {:?}",
            a.status
        );
        a.apply(Command::EditSave); // explicit save: loud reason
        assert!(
            a.status
                .as_ref()
                .is_some_and(|(m, e)| *e && m.contains("need a name")),
            "status: {:?}",
            a.status
        );

        // Type a value: the script uppercases it on the way through.
        a.apply(Command::EditMove(1)); // back to name
        a.apply(Command::EditBegin);
        for c in "grace".chars() {
            a.apply(Command::EditChar(c));
        }
        a.apply(Command::EditSave);
        a.sync();
        let q = a.db.query("SELECT name FROM people").unwrap();
        assert_eq!(q.rows[0][0], PValue::Text("GRACE".into()));
    }

    /// #12: the `edit` command opens the multi-line editor; typing,
    /// Enter, and F6 round-trip a real script.
    #[test]
    fn script_editor_edits_and_saves() {
        let mut a = app();
        for c in "edit t OnValidate".chars() {
            a.apply(Command::PromptChar(c));
        }
        a.apply(Command::PromptRun);
        assert!(matches!(a.overlay, Overlay::ScriptEditor(_)));
        for c in "say(\"hi\")".chars() {
            a.apply(Command::ScriptChar(c));
        }
        a.apply(Command::ScriptNewline);
        for c in "return 1".chars() {
            a.apply(Command::ScriptChar(c));
        }
        a.apply(Command::ScriptSave);
        assert!(matches!(a.overlay, Overlay::None));
        let src = crate::script::get_script(a.db.link(), "t", "OnValidate").unwrap();
        assert_eq!(src, "say(\"hi\")\nreturn 1");
    }

    /// #12: the editor edits a menu item's script (multi-line), returns
    /// to the Applications Generator, and the item runs the new source.
    #[test]
    fn menu_item_script_editor_round_trips() {
        let (db, _) = EmbeddedDb::open(":memory:").unwrap();
        db.execute("CREATE TABLE log(x TEXT)").unwrap();
        let mut a = App::new(Box::new(db), None);
        appsgen::ensure_app(a.db.link(), "demo").unwrap();
        appsgen::add_item(a.db.link(), "demo", "Do it").unwrap();
        let mut items = appsgen::items(a.db.link(), "demo");
        items[0].kind = ActionKind::Script;
        items[0].action_ref = String::new();
        appsgen::update_item(a.db.link(), &items[0]).unwrap();

        a.apply(Command::OpenApps(Some("demo".into())));
        a.apply(Command::OpenSelectedScript);
        assert!(
            matches!(&a.overlay, Overlay::ScriptEditor(st)
                if matches!(st.target, ScriptTarget::MenuItem { .. })),
            "editor targets the menu item"
        );
        for c in "execute(\"insert into log values ('a')\")".chars() {
            a.apply(Command::ScriptChar(c));
        }
        a.apply(Command::ScriptNewline);
        for c in "ui.refresh()".chars() {
            a.apply(Command::ScriptChar(c));
        }
        a.apply(Command::ScriptSave);
        assert!(matches!(a.overlay, Overlay::Apps(_)), "returns to designer");
        let items = appsgen::items(a.db.link(), "demo");
        assert!(
            items[0].action_ref.contains('\n'),
            "{:?}",
            items[0].action_ref
        );

        // Run it: designer → live menu → the item.
        a.apply(Command::DesignerRun);
        assert!(matches!(a.overlay, Overlay::AppMenu(_)));
        a.apply(Command::DesignerRun);
        a.sync();
        let q = a.db.query("SELECT count(*) FROM log").unwrap();
        assert_eq!(q.rows[0][0], PValue::Int(1), "multi-line script ran");
    }

    /// Backspace at column 0 joins with the previous line.
    #[test]
    fn script_editor_backspace_joins_lines() {
        let mut a = app();
        a.apply(Command::OpenFormScript {
            table: "t".into(),
            event: "OnSave".into(),
        });
        for c in "ab".chars() {
            a.apply(Command::ScriptChar(c));
        }
        a.apply(Command::ScriptNewline);
        for c in "cd".chars() {
            a.apply(Command::ScriptChar(c));
        }
        a.apply(Command::ScriptBackspace); // drop 'd'
        a.apply(Command::ScriptBackspace); // drop 'c'
        a.apply(Command::ScriptBackspace); // join onto "ab"
        a.apply(Command::ScriptSave);
        assert_eq!(
            crate::script::get_script(a.db.link(), "t", "OnSave").as_deref(),
            Some("ab")
        );
    }

    /// #12: `ui.*` effects from a script are dispatched as bus commands.
    #[test]
    fn script_effects_drive_the_bus() {
        let mut a = app();
        appsgen::ensure_app(a.db.link(), "demo").unwrap();
        appsgen::add_item(a.db.link(), "demo", "Open t").unwrap();
        let mut items = appsgen::items(a.db.link(), "demo");
        items[0].kind = ActionKind::Script;
        items[0].action_ref = "ui.browse(\"t\")".into();
        appsgen::update_item(a.db.link(), &items[0]).unwrap();
        a.apply(Command::OpenAppMenu(Some("demo".into())));
        a.apply(Command::DesignerRun);
        a.sync();
        assert!(
            matches!(
                &a.grid,
                Some(Grid { source: GridSource::Table { name, .. }, .. }) if name == "t"
            ),
            "ui.browse opened table t"
        );
    }

    /// #12: OnChange fires when a field is committed (Enter/Tab).
    #[test]
    fn form_change_script_fires_on_commit() {
        let (db, _) = EmbeddedDb::open(":memory:").unwrap();
        db.execute(
            "CREATE TABLE people(id INTEGER PRIMARY KEY, name TEXT);
             INSERT INTO people(name) VALUES ('ada');",
        )
        .unwrap();
        let mut a = App::new(Box::new(db), None);
        let bind = r#"script people OnChange if field == "name" then set("name", string.upper(record.name)) end"#;
        for c in bind.chars() {
            a.apply(Command::PromptChar(c));
        }
        a.apply(Command::PromptRun);
        a.sync();

        a.apply(Command::SidebarSeek('p'));
        a.apply(Command::OpenSelected);
        a.sync();
        a.apply(Command::OpenEdit);
        a.apply(Command::EditMove(1)); // name
        a.apply(Command::EditBegin);
        for c in "bob".chars() {
            a.apply(Command::EditChar(c));
        }
        a.apply(Command::EditCommitField); // OnChange runs, then Enter saves
        a.sync();
        let q = a.db.query("SELECT name FROM people").unwrap();
        assert_eq!(
            q.rows[0][0],
            PValue::Text("BOB".into()),
            "OnChange rewrote the field before save"
        );
    }

    /// #12: a menu item of kind `script` runs a sandboxed Lua action
    /// that talks to the db through the same DbLink.
    #[test]
    fn script_action_runs_through_the_menu() {
        let (db, _) = EmbeddedDb::open(":memory:").unwrap();
        db.execute("CREATE TABLE log(x TEXT)").unwrap();
        let mut a = App::new(Box::new(db), None);
        appsgen::ensure_app(a.db.link(), "demo").unwrap();
        appsgen::add_item(a.db.link(), "demo", "Log it").unwrap();
        let mut items = appsgen::items(a.db.link(), "demo");
        items[0].kind = ActionKind::Script;
        items[0].action_ref = "say(execute(\"insert into log values ('hi')\"))".into();
        appsgen::update_item(a.db.link(), &items[0]).unwrap();
        a.apply(Command::OpenAppMenu(Some("demo".into())));
        a.apply(Command::DesignerRun);
        a.sync();
        let q = a.db.query("SELECT count(*) FROM log").unwrap();
        assert_eq!(q.rows[0][0], PValue::Int(1), "script wrote the row");
        assert!(
            !a.status.as_ref().is_some_and(|(_, e)| *e),
            "status: {:?}",
            a.status
        );
    }

    /// #16: QBE cycles an FK join (`J`) and GROUP BY (`g`) through the
    /// bus, and the generated SQL actually runs into the grid.
    #[test]
    fn qbe_join_and_group_through_the_bus() {
        let (db, _) = EmbeddedDb::open(":memory:").unwrap();
        db.execute(
            "CREATE TABLE customers(id INTEGER PRIMARY KEY, name TEXT);
             CREATE TABLE orders(id INTEGER PRIMARY KEY,
                 customer_id INTEGER REFERENCES customers(id), amount REAL);
             INSERT INTO customers(name) VALUES ('Ada'), ('Grace');
             INSERT INTO orders(customer_id, amount) VALUES (1,10.0),(1,20.0),(2,5.0);",
        )
        .unwrap();
        let mut a = App::new(Box::new(db), None);
        a.apply(Command::OpenQbe(Some("orders".into())));
        a.apply(Command::DesignerJoin);
        let Overlay::Qbe(st) = &a.overlay else {
            panic!("qbe closed")
        };
        assert_eq!(
            st.spec.join.as_ref().map(|j| j.table.as_str()),
            Some("customers")
        );
        a.apply(Command::DesignerRun);
        a.sync();
        assert_eq!(a.grid.as_ref().unwrap().total, 3, "join keeps all orders");

        // Group by customer_id: one row per customer with a count.
        a.apply(Command::OpenQbe(Some("orders".into())));
        a.apply(Command::DesignerGroup); // first column = id? cycle to customer_id
        if let Overlay::Qbe(st) = &mut a.overlay {
            st.spec.group_by = Some("customer_id".into());
        }
        a.apply(Command::DesignerRun);
        a.sync();
        let g = a.grid.as_ref().unwrap();
        assert_eq!(g.total, 2, "two customer groups");
        assert_eq!(g.columns.last().map(String::as_str), Some("n"));
        let counts: Vec<i64> = g
            .cache
            .iter()
            .filter_map(|r| match r.last() {
                Some(PValue::Int(n)) => Some(*n),
                _ => None,
            })
            .collect();
        assert_eq!(counts, vec![2, 1]);
    }

    /// #8: report F6 prompts for a name; renaming retires the old row.
    #[test]
    fn report_save_as_names_and_renames() {
        let mut a = app();
        a.apply(Command::OpenReport(Some("t".into())));
        a.apply(Command::DesignerSave); // opens the name prompt
        for c in "monthly".chars() {
            a.apply(Command::DesignerChar(c));
        }
        a.apply(Command::DesignerCommit);
        let Overlay::Report(st) = &a.overlay else {
            panic!("designer closed")
        };
        assert_eq!(st.spec.name, "monthly");
        let names = crate::store::names(a.db.link(), "_phosphor_reports", "name");
        assert!(names.contains(&"monthly".to_owned()), "{names:?}");
        assert!(!names.contains(&"t".to_owned()), "{names:?}");

        // Rename again: the old catalog row is retired.
        a.apply(Command::DesignerSave);
        for c in "quarterly".chars() {
            a.apply(Command::DesignerChar(c));
        }
        a.apply(Command::DesignerCommit);
        let names = crate::store::names(a.db.link(), "_phosphor_reports", "name");
        assert!(names.contains(&"quarterly".to_owned()), "{names:?}");
        assert!(!names.contains(&"monthly".to_owned()), "{names:?}");
    }

    #[test]
    fn report_preview_through_the_bus() {
        let mut a = app();
        a.apply(Command::OpenReport(Some("t".into())));
        assert!(matches!(a.overlay, Overlay::Report(_)));
        a.apply(Command::DesignerRun); // preview
        let Overlay::Pager(p) = &a.overlay else {
            panic!("expected pager");
        };
        let text = p.lines.join("\n");
        assert!(text.contains("t report"), "page header with title");
        assert!(text.contains("TOTAL (500 rows)"), "grand totals");
        // The pk column is an identifier, not a quantity: its 1..=500
        // sum (125250) must NOT appear anywhere in the report.
        assert!(!text.contains("125250"), "pk column must not be totaled");
    }

    /// #17: the report designer's group-by cell accepts a typed SQL
    /// expression and the preview bands on it.
    #[test]
    fn report_group_by_expression_through_the_bus() {
        let (db, _) = EmbeddedDb::open(":memory:").unwrap();
        db.execute(
            "CREATE TABLE sales(region TEXT, amount REAL);
             INSERT INTO sales VALUES ('east',10.0),('east',20.0),('west',5.0);",
        )
        .unwrap();
        let mut a = App::new(Box::new(db), None);
        a.apply(Command::OpenReport(Some("sales".into())));
        // cursor 0 → title, 1 → source, 2 → group by
        a.apply(Command::DesignerMove(2));
        a.apply(Command::DesignerEditBegin);
        for c in "substr(region,1,1)".chars() {
            a.apply(Command::DesignerChar(c));
        }
        a.apply(Command::DesignerCommit);
        let Overlay::Report(st) = &a.overlay else {
            panic!("designer closed")
        };
        assert_eq!(st.spec.group_by.as_deref(), Some("substr(region,1,1)"));
        a.apply(Command::DesignerRun);
        let Overlay::Pager(p) = &a.overlay else {
            panic!("expected pager")
        };
        let text = p.lines.join("\n");
        assert!(text.contains("▌ substr(region,1,1) = e"), "{text}");
        assert!(text.contains("▌ substr(region,1,1) = w"), "{text}");
        assert!(text.contains("TOTAL (3 rows)"), "{text}");
    }

    /// #15: a PICTURE mask set in the form designer formats the live
    /// edit buffer and refuses a value that does not fit.
    #[test]
    fn picture_mask_through_the_bus() {
        let mut a = app();
        // Craft a mask on column b.
        a.apply(Command::OpenForm(Some("t".into())));
        a.apply(Command::DesignerMove(1));
        a.apply(Command::DesignerEditMask);
        for c in "999-99-9999".chars() {
            a.apply(Command::DesignerChar(c));
        }
        a.apply(Command::DesignerCommit);
        a.apply(Command::DesignerSave);
        a.apply(Command::Back);
        let spec = crate::forms::FormSpec::load(a.db.link(), "t").unwrap();
        assert_eq!(spec.fields[1].mask, "999-99-9999");

        // Editing formats as you type.
        a.apply(Command::OpenSelected);
        a.sync();
        a.apply(Command::OpenEdit);
        a.apply(Command::EditMove(1));
        a.apply(Command::EditBegin);
        for c in "123456789".chars() {
            a.apply(Command::EditChar(c));
        }
        let Overlay::Edit(ed) = &a.overlay else {
            panic!("edit closed")
        };
        assert_eq!(ed.editing.as_deref(), Some("123-45-6789"));
        a.apply(Command::EditCommitField);
        let q = a.db.query("SELECT b FROM t WHERE a = 1").unwrap();
        assert_eq!(q.rows[0][0], PValue::Text("123-45-6789".into()));

        // An incomplete value is refused.
        a.apply(Command::EditMove(1));
        a.apply(Command::EditBegin);
        for c in "12".chars() {
            a.apply(Command::EditChar(c));
        }
        a.apply(Command::EditCommitField);
        assert!(
            a.status
                .as_ref()
                .is_some_and(|(m, e)| *e && m.contains("must match")),
            "status: {:?}",
            a.status
        );
    }

    #[test]
    fn crafted_form_reorders_relabels_and_requires() {
        let mut a = app();
        // Craft a form for t: hide a, relabel b, make it required.
        a.apply(Command::OpenForm(Some("t".into())));
        let Overlay::Form(st) = &mut a.overlay else {
            panic!("form designer did not open");
        };
        st.spec.fields[0].include = false; // hide id column a
        st.spec.fields[1].label = "Row name".into();
        st.spec.fields[1].required = true;
        a.apply(Command::DesignerSave);
        a.apply(Command::Back);

        // EDIT now shows one field, custom label, and enforces required.
        a.apply(Command::OpenSelected);
        a.sync();
        a.apply(Command::OpenEdit);
        let Overlay::Edit(ed) = &a.overlay else {
            panic!("edit did not open");
        };
        assert_eq!(ed.fields.len(), 1, "hidden field is gone");
        assert_eq!(ed.labels[0], "Row name");
        assert!(ed.required[0]);
        // Blank the required field → save must refuse.
        a.apply(Command::EditBegin);
        if let Overlay::Edit(ed) = &mut a.overlay {
            ed.editing = Some(String::new());
        }
        a.apply(Command::EditCommitField);
        a.apply(Command::EditSave);
        assert!(
            matches!(a.overlay, Overlay::Edit(_)),
            "save must be refused while required field is blank"
        );
        assert!(a
            .status
            .as_ref()
            .is_some_and(|(m, err)| *err && m.contains("required")));
    }

    #[test]
    fn applications_generator_end_to_end() {
        let mut a = app();
        // Craft an app: one browse item pointing at t, one sql item.
        a.apply(Command::OpenApps(Some("crm".into())));
        a.apply(Command::DesignerAdd);
        a.apply(Command::DesignerEditBegin);
        if let Overlay::Apps(st) = &mut a.overlay {
            st.editing = Some("Rows".into());
        }
        a.apply(Command::DesignerCommit);
        a.apply(Command::DesignerEditAlt);
        if let Overlay::Apps(st) = &mut a.overlay {
            st.editing = Some("t".into());
        }
        a.apply(Command::DesignerCommit);
        // F2: designer → live menu.
        a.apply(Command::DesignerRun);
        assert!(matches!(a.overlay, Overlay::AppMenu(_)));
        // Hotkey 'r' (first letter of "Rows") runs the browse action.
        a.apply(Command::DesignerChar('r'));
        a.sync();
        assert!(matches!(a.overlay, Overlay::None));
        assert!(matches!(
            a.grid.as_ref().unwrap().source,
            GridSource::Table { .. }
        ));
        assert_eq!(a.grid.as_ref().unwrap().total, 500);
        // App-mode Esc-at-top returns to the menu.
        a.app_home = Some("crm".into());
        a.apply(Command::Back); // grid → sidebar
        a.apply(Command::Back); // sidebar → app menu (app mode)
        assert!(matches!(a.overlay, Overlay::AppMenu(_)));
    }

    #[test]
    fn insert_and_delete_rows_through_the_bus() {
        let mut a = app();
        a.apply(Command::OpenSelected);
        a.sync();
        assert_eq!(a.grid.as_ref().unwrap().total, 500);

        // INSERT: 'a' opens a NEW form; type into b; save.
        a.apply(Command::OpenInsert);
        let Overlay::Edit(ed) = &a.overlay else {
            panic!("insert form did not open")
        };
        assert!(ed.inserting);
        a.apply(Command::EditMove(1)); // to column b
        a.apply(Command::EditBegin);
        if let Overlay::Edit(ed) = &mut a.overlay {
            ed.editing = Some(String::new());
        }
        for c in "the 501st".chars() {
            a.apply(Command::EditChar(c));
        }
        a.apply(Command::EditCommitField);
        a.apply(Command::EditSave);
        assert!(matches!(a.overlay, Overlay::None));
        assert_eq!(a.grid.as_ref().unwrap().total, 501);

        // DELETE: first x arms, second x fires; a move in between disarms.
        a.apply(Command::GridBottom);
        a.apply(Command::DeleteRow);
        assert!(a.status.as_ref().is_some_and(|(m, _)| m.contains("again")));
        a.apply(Command::GridMove { dr: -1, dc: 0 }); // disarm
        a.apply(Command::DeleteRow); // re-arm on new row
        a.apply(Command::DeleteRow); // fire
        assert_eq!(a.grid.as_ref().unwrap().total, 500);
    }

    /// The optional `csv` keyword must be a standalone token: a table
    /// named `csvtest` was being parsed as `test` (caught by the UI reel).
    #[test]
    fn csv_keyword_is_not_a_table_prefix() {
        assert_eq!(
            strip_csv_keyword("csvtest /tmp/x.csv"),
            "csvtest /tmp/x.csv"
        );
        assert_eq!(
            strip_csv_keyword("csv people /tmp/x.csv"),
            "people /tmp/x.csv"
        );
        assert_eq!(
            strip_csv_keyword("CSV people /tmp/x.csv"),
            "people /tmp/x.csv"
        );
        assert_eq!(strip_csv_keyword("people /tmp/x.csv"), "people /tmp/x.csv");
    }

    /// End-to-end: `import csvtest path` fills a table whose name starts
    /// with `csv`.
    #[test]
    fn prompt_import_into_csv_prefixed_table() {
        let dir = std::env::temp_dir();
        let csv = dir.join(format!("phosphor-csvpref-{}.csv", std::process::id()));
        std::fs::write(&csv, "name\nAda\n").unwrap();
        let (db, _) = EmbeddedDb::open(":memory:").unwrap();
        db.execute("CREATE TABLE csvtest(id INTEGER PRIMARY KEY, name TEXT)")
            .unwrap();
        let mut a = App::new(Box::new(db), None);
        for c in format!("import csvtest {}", csv.display()).chars() {
            a.apply(Command::PromptChar(c));
        }
        a.apply(Command::PromptRun);
        a.sync();
        assert!(
            a.status
                .as_ref()
                .is_some_and(|(m, e)| !*e && m.contains("1 row(s)")),
            "status: {:?}",
            a.status
        );
        let q = a.db.query("SELECT count(*) FROM csvtest").unwrap();
        assert_eq!(q.rows[0][0], PValue::Int(1));
        let _ = std::fs::remove_file(&csv);
    }

    /// #11: Enter on an untouched NEW form advances but does not INSERT
    /// an all-NULL placeholder row.
    #[test]
    fn enter_on_untouched_new_form_does_not_insert() {
        let mut a = app();
        a.apply(Command::OpenSelected);
        a.sync();
        assert_eq!(a.grid.as_ref().unwrap().total, 500);
        a.apply(Command::OpenInsert);
        let n = match &a.overlay {
            Overlay::Edit(ed) => ed.fields.len(),
            _ => panic!("insert form did not open"),
        };
        for _ in 0..n {
            a.apply(Command::EditCommitField); // Enter with nothing typed
        }
        a.sync();
        assert_eq!(a.grid.as_ref().unwrap().total, 500, "no phantom row");
        assert!(
            matches!(&a.overlay, Overlay::Edit(ed) if ed.inserting),
            "form stays in NEW"
        );
        // Typing a value then Enter does insert.
        a.apply(Command::EditMove(1));
        a.apply(Command::EditBegin);
        for c in "real".chars() {
            a.apply(Command::EditChar(c));
        }
        a.apply(Command::EditCommitField);
        a.sync();
        assert_eq!(a.grid.as_ref().unwrap().total, 501);
    }

    /// #10: dropping the browsed table closes the zombie grid instead of
    /// leaving stale rows and quiet refill errors.
    #[test]
    fn dropping_the_browsed_table_closes_the_grid() {
        let mut a = app();
        a.apply(Command::OpenSelected);
        a.sync();
        assert!(a.grid.is_some());
        for c in "drop table t".chars() {
            a.apply(Command::PromptChar(c));
        }
        a.apply(Command::PromptRun);
        a.sync();
        assert!(a.grid.is_none(), "zombie grid must close");
        assert_eq!(a.focus, Focus::Sidebar);
    }

    #[test]
    fn find_scans_forward_and_repeats() {
        let mut a = app();
        a.apply(Command::OpenSelected);
        a.sync();
        for c in "find row437".chars() {
            a.apply(Command::PromptChar(c));
        }
        a.apply(Command::PromptRun);
        a.sync();
        assert_eq!(a.grid.as_ref().unwrap().cur_row, 436);
        // 'n' finds nothing further (unique value) and says so politely.
        a.apply(Command::FindNext);
        a.sync();
        assert!(a
            .status
            .as_ref()
            .is_some_and(|(m, _)| m.contains("not found")));
        assert_eq!(a.grid.as_ref().unwrap().cur_row, 436);
    }

    /// Find chains windows until the hit: needle past row 8192 needs
    /// three 4k windows (tests the async chain, not just one round).
    #[test]
    fn find_chains_windows_to_deep_hit() {
        let (db, _) = EmbeddedDb::open(":memory:").unwrap();
        db.execute(
            "CREATE TABLE big(id INTEGER PRIMARY KEY, v TEXT);
             INSERT INTO big(v)
               WITH RECURSIVE c(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM c WHERE x < 10000)
               SELECT 'item' || x FROM c;",
        )
        .unwrap();
        let mut a = App::new(Box::new(db), None);
        a.open_table("big");
        a.sync();
        assert_eq!(a.grid.as_ref().unwrap().total, 10_000);
        for c in "find item9000".chars() {
            a.apply(Command::PromptChar(c));
        }
        a.apply(Command::PromptRun);
        a.sync();
        assert!(a.pending.is_empty(), "chain completed");
        assert_eq!(a.grid.as_ref().unwrap().cur_row, 8999);
        assert!(a.status.as_ref().is_some_and(|(m, _)| m.contains("9000")));
    }

    /// A second find supersedes the first: stale chain responses die
    /// by seq guard, the latest needle wins.
    #[test]
    fn find_supersede_latest_wins() {
        let (db, _) = EmbeddedDb::open(":memory:").unwrap();
        db.execute(
            "CREATE TABLE big(id INTEGER PRIMARY KEY, v TEXT);
             INSERT INTO big(v)
               WITH RECURSIVE c(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM c WHERE x < 10000)
               SELECT 'item' || x FROM c;",
        )
        .unwrap();
        let mut a = App::new(Box::new(db), None);
        a.open_table("big");
        a.sync();
        // Early-hit needle first, then a deep one with no sync between:
        // both chains queue, but only the latest may land.
        a.find("item100");
        a.find("item9000");
        a.sync();
        assert!(a.pending.is_empty());
        assert_eq!(a.grid.as_ref().unwrap().cur_row, 8999);
    }

    /// Parked EDIT builds when its window arrives: cursor deep with a
    /// cold cache parks the form; the install triggers the build.
    #[test]
    fn parked_edit_builds_on_window_arrival() {
        let mut a = app();
        a.apply(Command::OpenSelected);
        a.sync();
        // Move the cursor deep WITHOUT fetching (bypass ensure): the
        // rows genuinely aren't here, so the form must park.
        if let Some(g) = &mut a.grid {
            g.cur_row = 499;
            g.row_off = 450;
        }
        a.build_edit_for(499);
        assert!(!matches!(a.overlay, Overlay::Edit(_)), "form parks");
        assert_eq!(a.pending_edit, Some(499));
        a.sync();
        let Overlay::Edit(ed) = &a.overlay else {
            panic!("parked form never built");
        };
        assert_eq!(ed.row_abs, 499);
        assert_eq!(a.pending_edit, None);
    }

    /// Rapid page flight converges: ten big jumps queue/overwrite
    /// windows, the last one wins and the cache matches the cursor.
    #[test]
    fn rapid_page_flight_converges() {
        let mut a = app();
        a.apply(Command::OpenSelected);
        a.sync();
        for _ in 0..10 {
            a.apply(Command::GridPage(1));
        }
        a.sync();
        assert!(a.pending_page.is_none(), "windows settled");
        let g = a.grid.as_ref().unwrap();
        assert!(
            g.row(g.cur_row).is_some(),
            "cache coherent with cursor at {}",
            g.cur_row
        );
        assert_eq!(g.total, 500);
    }

    /// 'v' opens the detail pane linked to the master cursor: Ada's
    /// orders on screen; moving to Grace re-links (the whole point).
    #[test]
    fn split_browse_links_master_to_detail() {
        let (db, _) = EmbeddedDb::open(":memory:").unwrap();
        db.execute(
            "CREATE TABLE customers(id INTEGER PRIMARY KEY, name TEXT);
             CREATE TABLE orders(id INTEGER PRIMARY KEY, product TEXT,
                                 customer_id INTEGER REFERENCES customers(id));
             INSERT INTO customers(name) VALUES ('Ada'), ('Grace');
             INSERT INTO orders(product, customer_id)
               VALUES ('modem', 1), ('coax', 1), ('router', 2);",
        )
        .unwrap();
        let mut a = App::new(Box::new(db), None);
        a.apply(Command::OpenSelected); // customers first
        a.sync();
        a.visible_cols_width = 120; // wide enough to split
        a.apply(Command::ToggleSplit);
        a.sync();
        let d = a.detail.as_ref().expect("detail pane opened");
        let GridSource::Detail { child, key_sql, .. } = &d.grid.source else {
            panic!("detail source");
        };
        assert_eq!(child, "orders");
        assert_eq!(key_sql, "1", "filtered to Ada (rowid 1)");
        assert_eq!(d.grid.total, 2, "Ada's two orders");
        // Cursor down to Grace: the pane re-links automatically.
        a.apply(Command::GridMove { dr: 1, dc: 0 });
        a.sync();
        let d = a.detail.as_ref().unwrap();
        let GridSource::Detail { key_sql, .. } = &d.grid.source else {
            panic!("detail source");
        };
        assert_eq!(key_sql, "2", "re-linked to Grace");
        assert_eq!(d.grid.total, 1);
        assert_eq!(d.grid.cache[0][1], PValue::Text("router".into()));
    }

    /// 'v' closes an open pane (single link); on a narrow terminal it
    /// refuses politely instead of squeezing two grids into 80 cols.
    #[test]
    fn split_toggles_closed_and_refuses_narrow() {
        let mut a = app(); // t(a,b): no FKs at all
        a.apply(Command::OpenSelected);
        a.sync();
        a.visible_cols_width = 120;
        a.apply(Command::ToggleSplit);
        assert!(a.detail.is_none(), "no declared FKs: nothing to show");
        assert!(a
            .status
            .as_ref()
            .is_some_and(|(m, _)| m.contains("no related")));

        // FK fixture, but narrow: refuses with a hint.
        let (db, _) = EmbeddedDb::open(":memory:").unwrap();
        db.execute(
            "CREATE TABLE customers(id INTEGER PRIMARY KEY, name TEXT);
             CREATE TABLE orders(id INTEGER PRIMARY KEY, customer_id INTEGER REFERENCES customers(id));
             INSERT INTO customers VALUES (1, 'Ada');",
        )
        .unwrap();
        let mut b = App::new(Box::new(db), None);
        b.apply(Command::OpenSelected);
        b.sync();
        b.visible_cols_width = 70;
        b.apply(Command::ToggleSplit);
        assert!(b.detail.is_none());
        assert!(b.status.as_ref().is_some_and(|(m, _)| m.contains("wider")));
        // Widen: now it opens, and 'v' again closes it.
        b.visible_cols_width = 120;
        b.apply(Command::ToggleSplit);
        b.sync();
        assert!(b.detail.is_some());
        b.apply(Command::ToggleSplit);
        assert!(b.detail.is_none());
        assert_eq!(b.focus, Focus::Grid);
    }

    /// Esc while the detail pane has focus closes the pane, not the grid.
    #[test]
    fn back_from_detail_pane_keeps_grid() {
        let (db, _) = EmbeddedDb::open(":memory:").unwrap();
        db.execute(
            "CREATE TABLE customers(id INTEGER PRIMARY KEY, name TEXT);
             CREATE TABLE orders(id INTEGER PRIMARY KEY, customer_id INTEGER REFERENCES customers(id));
             INSERT INTO customers VALUES (1, 'Ada');",
        )
        .unwrap();
        let mut a = App::new(Box::new(db), None);
        a.apply(Command::OpenSelected);
        a.sync();
        a.visible_cols_width = 120;
        a.apply(Command::ToggleSplit);
        a.sync();
        a.apply(Command::Focus(Focus::Detail));
        a.apply(Command::Back);
        assert!(a.detail.is_none(), "pane closed");
        assert!(a.grid.is_some(), "master stays");
        assert_eq!(a.focus, Focus::Grid);
    }

    /// Mouse clicks route through the bus: sidebar select (re-click
    /// opens), master row placement, detail focus + row, wheel scroll.
    #[test]
    fn mouse_clicks_and_wheel_follow_the_bus() {
        let (db, _) = EmbeddedDb::open(":memory:").unwrap();
        db.execute(
            "CREATE TABLE customers(id INTEGER PRIMARY KEY, name TEXT);
             CREATE TABLE orders(id INTEGER PRIMARY KEY, product TEXT,
                                 customer_id INTEGER REFERENCES customers(id));
             INSERT INTO customers(name) VALUES ('Ada'), ('Grace');
             INSERT INTO orders(product, customer_id)
               VALUES ('modem', 1), ('coax', 1), ('router', 2);",
        )
        .unwrap();
        let mut a = App::new(Box::new(db), None);
        a.apply(Command::OpenSelected);
        a.sync();
        // Fake the renderer's hit rects: sidebar rows start at y=1,
        // master content at y=1 (header at y=0).
        use ratatui::layout::Rect;
        a.hit = HitRects {
            sidebar: Some(Rect {
                x: 0,
                y: 1,
                width: 22,
                height: 10,
            }),
            master: Some(Rect {
                x: 25,
                y: 1,
                width: 40,
                height: 20,
            }),
            detail: None,
            prompt: Some(Rect {
                x: 0,
                y: 28,
                width: 100,
                height: 1,
            }),
        };
        use ratatui::crossterm::event::MouseEventKind as K;
        // Click sidebar row 2 (orders): selects, second click opens.
        a.on_mouse(&K::Down(ratatui::crossterm::event::MouseButton::Left), 3, 2);
        assert_eq!(a.sidebar_idx, 1);
        assert_eq!(a.focus, Focus::Sidebar);
        a.on_mouse(&K::Down(ratatui::crossterm::event::MouseButton::Left), 3, 2);
        a.sync();
        assert!(matches!(
            a.grid.as_ref().unwrap().source,
            GridSource::Table { .. }
        ));
        // Refresh rects (draw would): click master row 0 (Ada, y=2 is
        // the first data row — y=1 is the header), then row 1 (Grace).
        a.hit.master = Some(Rect {
            x: 25,
            y: 1,
            width: 40,
            height: 20,
        });
        a.on_mouse(
            &K::Down(ratatui::crossterm::event::MouseButton::Left),
            30,
            2,
        );
        assert_eq!(a.grid.as_ref().unwrap().cur_row, 0);
        assert_eq!(a.focus, Focus::Grid);
        a.on_mouse(
            &K::Down(ratatui::crossterm::event::MouseButton::Left),
            30,
            3,
        );
        assert_eq!(a.grid.as_ref().unwrap().cur_row, 1);
        // Wheel scrolls the master.
        a.on_mouse(&K::ScrollDown, 30, 2);
        assert_eq!(a.grid.as_ref().unwrap().cur_row, 2);
        a.on_mouse(&K::ScrollUp, 30, 2);
        assert_eq!(a.grid.as_ref().unwrap().cur_row, 1);
        // Click prompt focuses it.
        a.on_mouse(
            &K::Down(ratatui::crossterm::event::MouseButton::Left),
            50,
            28,
        );
        assert_eq!(a.focus, Focus::Prompt);
    }

    /// Slice C: a split choice is remembered in _phosphor_prefs and
    /// restored on the next open — even from a whole new App (the
    /// 1988 "my screen comes back tomorrow" contract). Link order is
    /// alphabetical (notes < orders), and the test stays order-agnostic.
    #[test]
    fn split_layout_is_remembered_across_sessions() {
        let file = std::env::temp_dir().join(format!("phosphor-split-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&file);
        let path = file.to_str().unwrap().to_owned();
        let (db1, _) = EmbeddedDb::open(&path).unwrap();
        db1.execute(
            "CREATE TABLE customers(id INTEGER PRIMARY KEY, name TEXT);
             CREATE TABLE orders(id INTEGER PRIMARY KEY, customer_id INTEGER REFERENCES customers(id));
             CREATE TABLE notes(id INTEGER PRIMARY KEY, customer_id INTEGER REFERENCES customers(id));
             INSERT INTO customers VALUES (1, 'Ada');",
        )
        .unwrap();
        let mut a = App::new(Box::new(db1), None);
        a.apply(Command::OpenSelected);
        a.sync();
        a.visible_cols_width = 120;
        a.apply(Command::ToggleSplit); // opens the FIRST link
        a.sync();
        let first_child = match &a.detail.as_ref().unwrap().grid.source {
            GridSource::Detail { child, .. } => child.clone(),
            _ => panic!("detail source"),
        };
        a.apply(Command::ToggleSplit); // cycles to the SECOND link
        a.sync();
        let second_child = match &a.detail.as_ref().unwrap().grid.source {
            GridSource::Detail { child, .. } => child.clone(),
            _ => panic!("detail source"),
        };
        assert_ne!(first_child, second_child, "two related tables cycle");

        // Session 2: fresh App on the same file — 'v' restores the
        // remembered (second) link, not the alphabetical default.
        let (db2, _) = EmbeddedDb::open(&path).unwrap();
        let mut b = App::new(Box::new(db2), None);
        b.apply(Command::OpenSelected);
        b.sync();
        b.visible_cols_width = 120;
        b.apply(Command::ToggleSplit);
        b.sync();
        let d = b.detail.as_ref().expect("remembered split restored");
        let GridSource::Detail { child, .. } = &d.grid.source else {
            panic!()
        };
        assert_eq!(child, &second_child, "remembered link wins");
        let _ = std::fs::remove_file(&file);
    }

    /// Generality check: nothing about the split is customer/order
    /// specific. Different domain, FK targeting a NON-pk UNIQUE column,
    /// no "id" column anywhere.
    #[test]
    fn split_works_on_any_declared_fk_pair() {
        let (db, _) = EmbeddedDb::open(":memory:").unwrap();
        db.execute(
            "CREATE TABLE albums(name TEXT UNIQUE NOT NULL, artist TEXT);
             CREATE TABLE tracks(title TEXT, album_name TEXT REFERENCES albums(name), seconds INTEGER);
             INSERT INTO albums VALUES ('Kind of Blue', 'Miles Davis');
             INSERT INTO tracks VALUES ('So What', 'Kind of Blue', 545),
               ('Freddie Freeloader', 'Kind of Blue', 589);",
        )
        .unwrap();
        let mut a = App::new(Box::new(db), None);
        a.apply(Command::SidebarSeek('c')); // → customers (aaa/bbb/ccc don't match 'c')
        a.apply(Command::OpenSelected);
        a.sync();
        a.visible_cols_width = 120;
        a.apply(Command::ToggleSplit);
        a.sync();
        let d = a.detail.as_ref().expect("detail on albums");
        let GridSource::Detail { child, key_sql, .. } = &d.grid.source else {
            panic!("detail source");
        };
        assert_eq!(child, "tracks");
        assert_eq!(key_sql, "'Kind of Blue'", "non-pk FK target, quoted text");
        assert_eq!(d.grid.total, 2, "both tracks of the album");
    }

    /// Three related tables: 'v' must visit ALL of them (fronted by
    /// the remembered link) before closing — the cycle order is frozen
    /// at open, so pref rewrites can't reorder it mid-flight.
    #[test]
    fn split_cycle_visits_every_link_then_closes() {
        let (db, _) = EmbeddedDb::open(":memory:").unwrap();
        db.execute(
            "CREATE TABLE customers(id INTEGER PRIMARY KEY, name TEXT);
             CREATE TABLE aaa(id INTEGER PRIMARY KEY, cid INTEGER REFERENCES customers(id));
             CREATE TABLE bbb(id INTEGER PRIMARY KEY, cid INTEGER REFERENCES customers(id));
             CREATE TABLE ccc(id INTEGER PRIMARY KEY, cid INTEGER REFERENCES customers(id));
             INSERT INTO customers VALUES (1, 'Ada');
             INSERT INTO aaa(cid) VALUES (1);
             INSERT INTO bbb(cid) VALUES (1);
             INSERT INTO ccc(cid) VALUES (1);",
        )
        .unwrap();
        let mut a = App::new(Box::new(db), None);
        a.sidebar_idx = a
            .visible_tables()
            .iter()
            .position(|t| t.name == "customers")
            .unwrap();
        a.apply(Command::OpenSelected);
        a.sync();
        a.visible_cols_width = 120;
        let child_of = |a: &App| match &a.detail.as_ref().unwrap().grid.source {
            GridSource::Detail { child, .. } => child.clone(),
            _ => panic!("detail"),
        };
        a.apply(Command::ToggleSplit); // aaa (alphabetical first)
        a.sync();
        assert_eq!(child_of(&a), "aaa");
        a.apply(Command::ToggleSplit); // bbb (pref rewrite must not reorder)
        a.sync();
        a.apply(Command::ToggleSplit); // ccc — reachable!
        a.sync();
        assert_eq!(child_of(&a), "ccc");
        a.apply(Command::ToggleSplit); // close
        assert!(a.detail.is_none());
        a.apply(Command::ToggleSplit); // reopen: remembered = ccc fronts
        a.sync();
        assert_eq!(child_of(&a), "ccc");
    }

    #[test]
    fn prompt_completion_and_line_editing() {
        let mut a = app();
        // Unique table-name completion: "select * from t" is the goal.
        for c in "hea".chars() {
            a.apply(Command::PromptChar(c));
        }
        a.apply(Command::PromptComplete);
        assert_eq!(a.prompt.input, "health");
        // Ctrl-U clears; Ctrl-W deletes a word.
        a.apply(Command::PromptClear);
        assert!(a.prompt.input.is_empty());
        for c in "select one two".chars() {
            a.apply(Command::PromptChar(c));
        }
        a.apply(Command::PromptDeleteWord);
        assert_eq!(a.prompt.input, "select one ");
    }

    #[test]
    fn prompt_cursor_is_boundary_safe_on_multibyte() {
        let mut a = app();
        for c in "héllo ∅".chars() {
            a.apply(Command::PromptChar(c));
        }
        assert_eq!(a.prompt.input, "héllo ∅");
        // Byte cursor sits at the end (8 chars, 9 bytes).
        assert_eq!(a.prompt.cursor, a.prompt.input.len());
        a.apply(Command::PromptMove(-1));
        a.apply(Command::PromptBackspace);
        assert_eq!(a.prompt.input, "héllo∅");
        assert!(a.prompt.input.is_char_boundary(a.prompt.cursor));
        a.apply(Command::PromptDeleteWord);
        // Erases back to whitespace; the ∅ past the cursor survives.
        assert_eq!(a.prompt.input, "∅");
        assert_eq!(a.prompt.cursor, 0);
    }

    /// Async machinery, manually reconciled (no sync()): two rapid
    /// opens converge on the latest table; unknown tags are ignored.
    #[test]
    fn rapid_reopens_converge_on_latest() {
        let (db, _) = EmbeddedDb::open(":memory:").unwrap();
        db.execute("CREATE TABLE one(x); INSERT INTO one VALUES (1);")
            .unwrap();
        db.execute("CREATE TABLE two(x); INSERT INTO two VALUES (2);")
            .unwrap();
        let mut a = App::new(Box::new(db), None);
        a.open_table("one");
        a.open_table("two");
        assert_eq!(a.pending.len(), 2, "both jobs queued");
        // Main-loop style: non-blocking pumps until drained.
        for _ in 0..100 {
            a.pump();
            if a.pending.is_empty() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert!(a.pending.is_empty(), "worker answered everything");
        let g = a.grid.as_ref().expect("grid installed");
        match &g.source {
            GridSource::Table { name, .. } => assert_eq!(name, "two"),
            _ => panic!("wrong grid source"),
        }
        assert_eq!(g.total, 1);
        // Unknown tags (superseded/already handled) are ignored.
        let before = a.status.clone();
        a.apply(Command::DbReady(
            999_999,
            std::time::Duration::ZERO,
            crate::worker::DbResponse::Query(Err("stale".into())),
        ));
        assert_eq!(a.status, before);
    }

    #[test]
    fn fresh_app_needs_first_draw_and_commands_dirty_it() {
        let mut a = app();
        assert!(a.dirty, "first frame must paint");
        a.dirty = false;
        a.apply(Command::PromptClear);
        assert!(a.dirty, "apply() always dirties (main.rs redraw contract)");
    }

    #[test]
    fn theme_command() {
        let mut a = app();
        for c in "set theme amber".chars() {
            a.apply(Command::PromptChar(c));
        }
        a.apply(Command::PromptRun);
        a.sync();
        assert_eq!(a.theme.name, "amber");
    }

    /// #22: theme and shimmer persist in _phosphor_prefs and come back
    /// for the next session on the same file.
    #[test]
    fn theme_and_shimmer_persist_across_sessions() {
        let file = std::env::temp_dir().join(format!("phosphor-prefs-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&file);
        let path = file.to_str().unwrap().to_owned();
        {
            let (db, _) = EmbeddedDb::open(&path).unwrap();
            let mut a = App::new(Box::new(db), None);
            for c in "set theme paper".chars() {
                a.apply(Command::PromptChar(c));
            }
            a.apply(Command::PromptRun);
            a.sync();
            assert_eq!(a.theme.name, "paper");
            for c in "set shimmer on".chars() {
                a.apply(Command::PromptChar(c));
            }
            a.apply(Command::PromptRun);
            a.sync();
            assert!(a.shimmer);
        }
        let (db2, _) = EmbeddedDb::open(&path).unwrap();
        let b = App::new(Box::new(db2), None);
        assert_eq!(b.theme.name, "paper", "theme remembered");
        assert!(b.shimmer, "shimmer remembered");
        let _ = std::fs::remove_file(&file);
    }

    #[test]
    fn help_is_context_sensitive() {
        let mut a = app();
        // From the QBE designer, F1 lands on the QBE topic.
        a.apply(Command::OpenQbe(Some("t".into())));
        a.apply(Command::Help);
        let Overlay::Help(st) = &a.overlay else {
            panic!("help did not open");
        };
        assert_eq!(crate::help::TOPICS[st.topic].key, "qbe");
        // Topics cycle; scroll clamps at zero.
        a.apply(Command::HelpTopic(1));
        a.apply(Command::HelpScroll(-5));
        let Overlay::Help(st) = &a.overlay else {
            panic!()
        };
        assert_eq!(crate::help::TOPICS[st.topic].key, "reports");
        assert_eq!(st.scroll, 0);
        // Esc closes back toward where the user was.
        a.apply(Command::Back);
        assert!(matches!(a.overlay, Overlay::None));
        // From the prompt, F1 lands on the prompt topic.
        a.apply(Command::Focus(Focus::Prompt));
        a.apply(Command::Help);
        let Overlay::Help(st) = &a.overlay else {
            panic!()
        };
        assert_eq!(crate::help::TOPICS[st.topic].key, "prompt");
    }

    /// Full-stack: the LIVE console samples on tick without keys.
    #[test]
    fn health_console_ticks_a_sample_when_due() {
        let ext = "../timeless-libsql/target/release/libdbhealth_ext.so";
        if !std::path::Path::new(ext).exists() {
            eprintln!("skipping: {ext} not built");
            return;
        }
        std::env::set_var("PHOSPHOR_EXT", ext);
        let (db, warn) = crate::db::EmbeddedDb::open(":memory:").unwrap();
        assert!(warn.is_none(), "extension failed to load: {warn:?}");
        db.execute("CREATE VIRTUAL TABLE dbhealth USING timeless_health")
            .unwrap();
        db.execute("INSERT INTO dbhealth(dbhealth) VALUES ('sample')")
            .unwrap();
        let mut a = App::new(Box::new(db), None);
        a.apply(Command::OpenHealth);
        a.sync();
        assert!(matches!(a.overlay, Overlay::Health(_)));
        let count = |a: &App| -> i64 {
            match a.db.query("SELECT count(*) FROM dbhealth").unwrap().rows[0][0] {
                PValue::Int(n) => n,
                _ => panic!(),
            }
        };
        let before = count(&a);
        a.tick(); // not due yet: opening reset the clock
        assert_eq!(count(&a), before, "tick must respect the 5s cadence");
        // Time-travel the clock 6 seconds into the past and tick again.
        a.last_auto_sample = std::time::Instant::now()
            .checked_sub(std::time::Duration::from_secs(6))
            .unwrap();
        a.tick();
        assert!(count(&a) > before, "due tick takes a sample");
        assert!(
            matches!(a.overlay, Overlay::Health(_)),
            "console stays open"
        );
    }

    #[test]
    fn sidebar_hides_internals_and_seeks_by_letter() {
        let mut a = app();
        a.db
            .execute("CREATE TABLE zeta(x); CREATE TABLE t_chunks(x); CREATE TABLE _phosphor_apps(id INTEGER)")
            .unwrap();
        a.apply(Command::Refresh);
        let names: Vec<String> = a.visible_tables().iter().map(|t| t.name.clone()).collect();
        assert!(names.contains(&"t".into()) && names.contains(&"zeta".into()));
        assert!(
            !names
                .iter()
                .any(|n| n == "t_chunks" || n == "_phosphor_apps"),
            "internals hidden by default: {names:?}"
        );
        // First-letter seek jumps to zeta.
        a.apply(Command::SidebarSeek('z'));
        let vis = a.visible_tables();
        assert_eq!(vis[a.sidebar_idx].name, "zeta");
        // Toggle reveals everything.
        a.apply(Command::ToggleInternals);
        let all: Vec<String> = a.visible_tables().iter().map(|t| t.name.clone()).collect();
        assert!(all.iter().any(|n| n == "_phosphor_apps"), "{all:?}");
    }

    /// #7: an app can be named/renamed with `r`; items survive the rename.
    #[test]
    fn app_can_be_named_and_renamed() {
        let mut a = app();
        a.apply(Command::OpenApps(None)); // default name "app"
        a.apply(Command::RenameApp);
        for c in "crm".chars() {
            a.apply(Command::DesignerChar(c));
        }
        a.apply(Command::DesignerCommit);
        let Overlay::Apps(st) = &a.overlay else {
            panic!("designer closed")
        };
        assert_eq!(st.app, "crm");
        assert_eq!(appsgen::list_apps(a.db.link()), ["crm"]);
        a.apply(Command::DesignerAdd); // an item under crm
        assert_eq!(appsgen::items(a.db.link(), "crm").len(), 1);

        a.apply(Command::RenameApp);
        for c in "sales".chars() {
            a.apply(Command::DesignerChar(c));
        }
        a.apply(Command::DesignerCommit);
        assert_eq!(appsgen::list_apps(a.db.link()), ["sales"]);
        assert_eq!(
            appsgen::items(a.db.link(), "sales").len(),
            1,
            "items follow"
        );
    }

    #[test]
    fn app_mode_pager_closes_back_to_the_menu() {
        let mut a = app();
        appsgen::ensure_app(a.db.link(), "demo").unwrap();
        appsgen::add_item(a.db.link(), "demo", "Totals").unwrap();
        let mut items = appsgen::items(a.db.link(), "demo");
        items[0].kind = ActionKind::Report;
        items[0].action_ref = "t".into();
        appsgen::update_item(a.db.link(), &items[0]).unwrap();

        a.app_home = Some("demo".into());
        a.apply(Command::OpenAppMenu(Some("demo".into())));
        a.apply(Command::DesignerRun); // run the report → pager
        assert!(matches!(a.overlay, Overlay::Pager(_)));
        a.apply(Command::Back); // Esc: home means home
        assert!(
            matches!(a.overlay, Overlay::AppMenu(_)),
            "menu-launched pager must return to the menu in app mode"
        );
    }

    #[test]
    fn edit_pages_through_records_and_commits_dirty_edits() {
        let mut a = app();
        a.apply(Command::OpenSelected);
        a.sync();
        a.apply(Command::OpenEdit);
        let Overlay::Edit(ed) = &a.overlay else {
            panic!()
        };
        assert_eq!(ed.row_abs, 0);
        assert_eq!(ed.fields[1].1, PValue::Text("row1".into()));

        // Page forward: same overlay, next record.
        a.apply(Command::EditPage(1));
        let Overlay::Edit(ed) = &a.overlay else {
            panic!("form closed")
        };
        assert_eq!(ed.row_abs, 1);
        assert_eq!(ed.fields[1].1, PValue::Text("row2".into()));
        assert_eq!(a.grid.as_ref().unwrap().cur_row, 1, "grid follows");

        // Dirty edit commits on page (the dBASE contract).
        a.apply(Command::EditMove(1));
        a.apply(Command::EditBegin);
        if let Overlay::Edit(ed) = &mut a.overlay {
            ed.editing = Some(String::new());
        }
        for c in "paged-save".chars() {
            a.apply(Command::EditChar(c));
        }
        a.apply(Command::EditCommitField);
        a.apply(Command::EditPage(1));
        let Overlay::Edit(ed) = &a.overlay else {
            panic!()
        };
        assert_eq!(ed.row_abs, 2);
        let q = a.db.query("SELECT b FROM t WHERE a = 2").unwrap();
        assert_eq!(q.rows[0][0], PValue::Text("paged-save".into()));

        // Fly to the end: clamped with a message, form stays open.
        for _ in 0..600 {
            a.apply(Command::EditPage(1));
        }
        a.sync();
        let Overlay::Edit(ed) = &a.overlay else {
            panic!()
        };
        assert_eq!(ed.row_abs, 499);
        a.apply(Command::EditPage(1));
        assert!(a.status.as_ref().is_some_and(|(m, _)| m == "last record"));

        // Paging is blocked while inserting a NEW record.
        a.apply(Command::Back);
        a.apply(Command::OpenInsert);
        a.apply(Command::EditPage(1));
        assert!(matches!(&a.overlay, Overlay::Edit(ed) if ed.inserting));
    }

    #[test]
    fn forms_and_designer_are_live_typing_begins_the_edit() {
        // EDIT: no Enter needed — typing replaces the current value.
        let mut a = app();
        a.apply(Command::OpenSelected);
        a.sync();
        a.apply(Command::OpenEdit);
        a.apply(Command::EditMove(1)); // b = "row1"
        for c in "live".chars() {
            a.apply(Command::EditType(c));
        }
        let Overlay::Edit(ed) = &a.overlay else {
            panic!()
        };
        assert_eq!(ed.editing.as_deref(), Some("live"), "typed straight in");
        a.apply(Command::Back);
        a.apply(Command::Back);
        // TABLE DESIGNER: letters name the field the cursor is on.
        a.apply(Command::OpenCreate(Some("g".into())));
        a.apply(Command::DesignerAdd);
        for c in "city".chars() {
            a.apply(Command::CreateType(c));
        }
        a.apply(Command::DesignerCommit);
        let Overlay::Create(st) = &a.overlay else {
            panic!()
        };
        assert_eq!(st.draft.fields[1].name, "city", "no Enter, no backspacing");
    }

    /// The TABLE EDITOR end to end: 'E' opens a table's live columns,
    /// add + rename + drop edits apply as ALTERs, and the data
    /// survives (rename keeps it, drop loses only its own column).
    #[test]
    fn table_editor_applies_alters_and_preserves_data() {
        let (db, _) = EmbeddedDb::open(":memory:").unwrap();
        db.execute(
            "CREATE TABLE clients(id INTEGER PRIMARY KEY, name TEXT, city TEXT, stale TEXT);
             INSERT INTO clients(name, city, stale) VALUES
               ('Ada', 'London', 'x'), ('Grace', 'Arlington', 'y');",
        )
        .unwrap();
        let mut a = App::new(Box::new(db), None);
        a.apply(Command::SidebarSeek('c'));
        a.apply(Command::OpenSelected);
        a.sync();
        a.apply(Command::OpenTableEditor);
        let Overlay::Create(st) = &a.overlay else {
            panic!("table editor did not open");
        };
        assert_eq!(st.draft.fields.len(), 4, "live columns preloaded");
        assert!(st.original.is_some());
        // The editor opens with the cursor on the LAST field (stale).
        // Edit plan: drop `stale`, rename `city` -> `locality`, add
        // `balance REAL`.
        a.apply(Command::DesignerDelete); // stale dropped; cursor now on city
        a.apply(Command::DesignerEditBegin);
        if let Overlay::Create(st) = &mut a.overlay {
            if let Some(buf) = &mut st.editing {
                *buf = "locality".into();
            }
        }
        a.apply(Command::DesignerCommit);
        a.apply(Command::DesignerAdd); // new field after locality
        a.apply(Command::DesignerEditBegin);
        if let Overlay::Create(st) = &mut a.overlay {
            if let Some(buf) = &mut st.editing {
                *buf = "balance".into();
            }
        }
        a.apply(Command::DesignerCommit);
        a.apply(Command::DesignerCycle); // TEXT -> REAL
        a.apply(Command::DesignerRun);
        a.sync();
        assert!(matches!(a.overlay, Overlay::None), "editor closed");
        let cols = a.db.columns("clients").unwrap();
        let names: Vec<&str> = cols.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(
            names,
            ["id", "name", "locality", "balance"],
            "renamed+added+dropped"
        );
        let q =
            a.db.query("SELECT name, locality FROM clients ORDER BY name")
                .unwrap();
        assert_eq!(q.rows[0][0], PValue::Text("Ada".into()));
        assert_eq!(
            q.rows[0][1],
            PValue::Text("London".into()),
            "data survived the rename"
        );
    }

    /// The editor declines a type change on an existing column with a
    /// clear message instead of a raw SQLite error.
    #[test]
    fn table_editor_rebuilds_type_change() {
        let (db, _) = EmbeddedDb::open(":memory:").unwrap();
        db.execute("CREATE TABLE things(id INTEGER PRIMARY KEY, size TEXT);")
            .unwrap();
        db.execute("INSERT INTO things(size) VALUES ('big')")
            .unwrap();
        let mut a = App::new(Box::new(db), None);
        a.apply(Command::OpenSelected);
        a.sync();
        a.apply(Command::OpenTableEditor);
        a.apply(Command::DesignerCycle); // TEXT -> REAL: a type change
        a.apply(Command::DesignerRun);
        a.sync();
        assert!(matches!(a.overlay, Overlay::None), "applied and closed");
        // The rebuild re-declared the column and preserved the row.
        let cols = a.db.columns("things").unwrap();
        assert_eq!(cols[1].decl_type.to_ascii_uppercase(), "REAL");
        let q = a.db.query("SELECT size FROM things").unwrap();
        assert_eq!(q.rows[0][0], PValue::Text("big".into()));
        assert!(a
            .status
            .as_ref()
            .is_some_and(|(m, _)| m.contains("applied")));
    }

    /// #15: a computed field shows a calculated, read-only value and is
    /// never written back.
    #[test]
    fn computed_field_is_readonly_and_accurate() {
        let (db, _) = EmbeddedDb::open(":memory:").unwrap();
        db.execute(
            "CREATE TABLE orders(id INTEGER PRIMARY KEY, qty INTEGER, price REAL);
             INSERT INTO orders(qty, price) VALUES (3, 4.0);",
        )
        .unwrap();
        let mut a = App::new(Box::new(db), None);
        // Craft a form with a computed column `total`.
        a.apply(Command::OpenForm(Some("orders".into())));
        if let Overlay::Form(st) = &mut a.overlay {
            st.spec.fields.push(crate::forms::FormField {
                column: "total".into(),
                label: "Total".into(),
                include: true,
                required: false,
                pos: None,
                width: 12,
                mask: String::new(),
                computed: "qty * price".into(),
            });
        }
        a.apply(Command::DesignerSave);
        a.apply(Command::Back);

        a.apply(Command::OpenSelected);
        a.sync();
        a.apply(Command::OpenEdit);
        let Overlay::Edit(ed) = &a.overlay else {
            panic!("edit closed")
        };
        let i = ed
            .fields
            .iter()
            .position(|(c, _)| c.name == "total")
            .unwrap();
        assert_eq!(ed.computed[i].as_deref(), Some("qty * price"));
        assert_eq!(ed.fields[i].1, PValue::Real(12.0), "3 * 4.0");
        // Navigation skips the computed field.
        a.apply(Command::EditMove(1)); // from id → qty
        let Overlay::Edit(ed) = &a.overlay else {
            panic!()
        };
        assert_eq!(ed.cursor, 1);
        // Typing at it is refused.
        if let Overlay::Edit(ed) = &mut a.overlay {
            ed.cursor = i;
        }
        a.apply(Command::EditType('9'));
        let Overlay::Edit(ed) = &a.overlay else {
            panic!()
        };
        assert!(ed.editing.is_none(), "computed field not editable");
        // Save writes qty/price only; `total` is not a column.
        a.apply(Command::EditSave);
        let err = a.db.query("SELECT total FROM orders").unwrap_err();
        assert!(err.contains("no such column"), "{err}");
    }

    /// #18: `H` flips the split orientation and it is remembered.
    #[test]
    fn split_orientation_toggles_and_persists() {
        let file =
            std::env::temp_dir().join(format!("phosphor-splitdir-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&file);
        let path = file.to_str().unwrap().to_owned();
        {
            let (db, _) = EmbeddedDb::open(&path).unwrap();
            db.execute(
                "CREATE TABLE customers(id INTEGER PRIMARY KEY, name TEXT);
                 CREATE TABLE orders(id INTEGER PRIMARY KEY,
                     customer_id INTEGER REFERENCES customers(id));
                 INSERT INTO customers VALUES (1, 'Ada');",
            )
            .unwrap();
            let mut a = App::new(Box::new(db), None);
            assert!(!a.split_horizontal, "default is side-by-side");
            a.apply(Command::ToggleSplitDir);
            assert!(a.split_horizontal);
            a.apply(Command::OpenSelected);
            a.sync();
            a.visible_rows = 20; // stacked needs height, not width
            a.apply(Command::ToggleSplit);
            a.sync();
            assert!(a.detail.is_some(), "stacked split opens");
        }
        let (db2, _) = EmbeddedDb::open(&path).unwrap();
        let b = App::new(Box::new(db2), None);
        assert!(b.split_horizontal, "orientation remembered");
        let _ = std::fs::remove_file(&file);
    }

    /// #21: `--readonly` refuses every write shape through the bus.
    #[test]
    fn readonly_kiosk_refuses_writes() {
        let mut a = app();
        a.readonly = true;
        a.apply(Command::OpenSelected);
        a.sync();
        // Insert form never opens.
        a.apply(Command::OpenInsert);
        assert!(matches!(a.overlay, Overlay::None), "insert blocked");
        assert!(a
            .status
            .as_ref()
            .is_some_and(|(m, e)| *e && m.contains("read-only")));
        // Delete blocked; nothing armed.
        a.apply(Command::DeleteRow);
        assert!(a.pending_delete.is_none());
        // A writing prompt statement is refused.
        for c in "insert into t(b) values ('x')".chars() {
            a.apply(Command::PromptChar(c));
        }
        a.apply(Command::PromptRun);
        a.sync();
        assert!(a
            .status
            .as_ref()
            .is_some_and(|(m, e)| *e && m.contains("read-only")));
        let n = a.db.query("SELECT count(*) FROM t").unwrap();
        assert_eq!(n.rows[0][0], PValue::Int(500), "no row written");
    }

    /// #15: F7 on an FK field opens a picker over the parent table and
    /// writes the chosen key into the field.
    #[test]
    fn fk_picker_fills_the_field() {
        let (db, _) = EmbeddedDb::open(":memory:").unwrap();
        db.execute(
            "CREATE TABLE customers(id INTEGER PRIMARY KEY, name TEXT);
             CREATE TABLE orders(id INTEGER PRIMARY KEY, product TEXT,
                                 customer_id INTEGER REFERENCES customers(id));
             INSERT INTO customers(name) VALUES ('Ada'), ('Grace');
             INSERT INTO orders(product) VALUES ('modem');",
        )
        .unwrap();
        let mut a = App::new(Box::new(db), None);
        // seek orders (only 'o' table), browse, edit the row
        a.apply(Command::SidebarSeek('o'));
        a.apply(Command::OpenSelected);
        a.sync();
        a.apply(Command::OpenEdit);
        a.apply(Command::EditMove(2)); // customer_id field
        a.apply(Command::EditPick);
        let Overlay::Edit(ed) = &a.overlay else {
            panic!("edit closed")
        };
        let p = ed.picker.as_ref().expect("picker opened");
        assert_eq!(p.rows.len(), 2);
        assert_eq!(p.columns[0], "id", "key column first");
        // Pick Grace (row 1) and commit.
        a.apply(Command::PickerMove(1));
        a.apply(Command::PickerCommit);
        let Overlay::Edit(ed) = &a.overlay else {
            panic!("edit closed")
        };
        assert!(ed.picker.is_none(), "picker closed");
        assert_eq!(ed.inputs[2].as_deref(), Some("2"), "Grace's id written");
        a.apply(Command::EditSave);
        let q = a.db.query("SELECT customer_id FROM orders").unwrap();
        assert_eq!(q.rows[0][0], PValue::Int(2));
    }

    #[test]
    fn declared_fks_become_live_child_panes() {
        let (db, _) = EmbeddedDb::open(":memory:").unwrap();
        db.execute(
            "CREATE TABLE customers(id INTEGER PRIMARY KEY, name TEXT);
             CREATE TABLE orders(id INTEGER PRIMARY KEY, product TEXT,
                                 customer_id INTEGER REFERENCES customers(id));
             INSERT INTO customers(name) VALUES ('Ada'), ('Grace');
             INSERT INTO orders(product, customer_id)
               VALUES ('modem', 1), ('coax', 1), ('router', 2);",
        )
        .unwrap();
        let mut a = App::new(Box::new(db), None);
        a.apply(Command::OpenSelected); // customers (alphabetical first)
        a.sync();
        a.apply(Command::OpenEdit); // Ada
        let Overlay::Edit(ed) = &a.overlay else {
            panic!()
        };
        assert_eq!(ed.links.len(), 1, "the declared FK is discovered");
        assert_eq!(ed.links[0].child, "orders");
        assert_eq!(ed.links[0].total, 2, "Ada has two orders");
        assert!(ed.links[0].rows.iter().any(|r| r.contains(&"coax".into())));
        // Paging to Grace refreshes the pane.
        a.apply(Command::EditPage(1));
        let Overlay::Edit(ed) = &a.overlay else {
            panic!()
        };
        assert_eq!(ed.links[0].total, 1, "Grace has one order");
        // Back to Ada: the pane comes from the per-key cache, same picture.
        a.apply(Command::EditPage(-1));
        let Overlay::Edit(ed) = &a.overlay else {
            panic!()
        };
        assert_eq!(ed.links[0].total, 2, "Ada again, cached");
        assert!(ed.links[0].rows.iter().any(|r| r.contains(&"modem".into())));
        // F4 jumps into a filtered BROWSE of the children.
        a.apply(Command::EditOpenLink(0));
        a.sync();
        let g = a.grid.as_ref().expect("filtered child browse");
        assert_eq!(g.total, 2, "only Ada's orders");
        // FKs are ENFORCED on the embedded backend now.
        let bad =
            a.db.execute("INSERT INTO orders(product, customer_id) VALUES ('x', 99)");
        assert!(bad.is_err(), "orphan insert must be rejected");
    }

    /// Esc from a parked-flip EDIT must stay closed: a window still in
    /// flight arrives later and must NOT resurrect the dismissed form.
    #[test]
    fn back_while_parked_keeps_edit_closed() {
        let mut a = app();
        a.apply(Command::OpenSelected);
        a.sync();
        a.apply(Command::OpenEdit);
        // Park a target (rows absent) as a flip would, then Esc.
        a.pending_edit = Some(400);
        a.apply(Command::Back);
        assert!(matches!(a.overlay, Overlay::None), "form closed");
        assert_eq!(a.pending_edit, None, "park must die with the form");
        // The late window arrives: nothing resurrects.
        a.apply(Command::GridPage(1));
        a.sync();
        assert!(matches!(a.overlay, Overlay::None));
    }

    #[test]
    fn quit_at_the_dot_prompt() {
        for word in ["quit", "QUIT", "exit", "q"] {
            let mut a = app();
            for c in word.chars() {
                a.apply(Command::PromptChar(c));
            }
            a.apply(Command::PromptRun);
            a.sync();
            assert!(a.quit, "{word:?} must quit");
        }
    }

    #[test]
    fn tab_moves_between_fields_and_folds_typing() {
        use ratatui::crossterm::event::{KeyCode, KeyEvent};
        let mut a = app();
        a.apply(Command::OpenSelected);
        a.sync();
        a.apply(Command::OpenEdit);
        // Idle: Tab advances a field, Shift-Tab returns.
        let cmd = a.map_key(KeyEvent::from(KeyCode::Tab)).unwrap();
        assert!(matches!(cmd, Command::EditMove(1)), "Tab must move fields");
        a.apply(cmd);
        let Overlay::Edit(ed) = &a.overlay else {
            panic!()
        };
        assert_eq!(ed.cursor, 1);
        // Mid-edit: Tab commits-and-advances; Shift-Tab keeps the text.
        a.apply(Command::EditBegin);
        for c in "kept".chars() {
            a.apply(Command::EditChar(c));
        }
        let cmd = a.map_key(KeyEvent::from(KeyCode::BackTab)).unwrap();
        assert!(matches!(cmd, Command::EditMove(-1)));
        a.apply(cmd);
        let Overlay::Edit(ed) = &a.overlay else {
            panic!()
        };
        assert_eq!(ed.cursor, 0, "Shift-Tab steps back");
        assert_eq!(
            ed.inputs[1].as_deref(),
            Some("kept"),
            "typing folded, not dropped"
        );
        let cmd = a.map_key(KeyEvent::from(KeyCode::Tab)).unwrap();
        a.apply(cmd);
        let Overlay::Edit(ed) = &a.overlay else {
            panic!()
        };
        assert_eq!(ed.cursor, 1, "Tab advances again");
    }

    #[test]
    fn f10_saves_while_still_typing_in_a_field() {
        let mut a = app();
        a.apply(Command::OpenSelected);
        a.sync();
        a.apply(Command::OpenEdit);
        a.apply(Command::EditMove(1)); // column b
        a.apply(Command::EditBegin);
        if let Overlay::Edit(ed) = &mut a.overlay {
            ed.editing = Some(String::new());
        }
        for c in "typed then F10".chars() {
            a.apply(Command::EditChar(c));
        }
        // NO EditCommitField — straight to save, like a human does it.
        a.apply(Command::EditSave);
        assert!(matches!(a.overlay, Overlay::None), "record saved+closed");
        let q = a.db.query("SELECT b FROM t WHERE a = 1").unwrap();
        assert_eq!(q.rows[0][0], PValue::Text("typed then F10".into()));
    }

    #[test]
    fn enter_saves_the_record_and_advances() {
        let mut a = app();
        a.apply(Command::OpenSelected);
        a.sync();
        a.apply(Command::OpenEdit);
        a.apply(Command::EditMove(1));
        a.apply(Command::EditBegin);
        if let Overlay::Edit(ed) = &mut a.overlay {
            ed.editing = Some(String::new());
        }
        for c in "enter-saved".chars() {
            a.apply(Command::EditChar(c));
        }
        a.apply(Command::EditCommitField); // Enter: commit + advance + SAVE
        assert!(matches!(a.overlay, Overlay::Edit(_)), "form stays open");
        let q = a.db.query("SELECT b FROM t WHERE a = 1").unwrap();
        assert_eq!(q.rows[0][0], PValue::Text("enter-saved".into()));
        let Overlay::Edit(ed) = &a.overlay else {
            panic!()
        };
        assert_eq!(ed.cursor, 0, "advanced (wrapped) to the next field");
        assert!(!ed.dirty(), "record is clean after the Enter-save");
    }

    #[test]
    fn enter_on_new_record_inserts_once_then_updates() {
        let mut a = app();
        a.apply(Command::OpenSelected);
        a.sync();
        a.apply(Command::OpenInsert);
        a.apply(Command::EditMove(1));
        a.apply(Command::EditBegin);
        if let Overlay::Edit(ed) = &mut a.overlay {
            ed.editing = Some(String::new());
        }
        for c in "once".chars() {
            a.apply(Command::EditChar(c));
        }
        a.apply(Command::EditCommitField); // Enter inserts...
        let Overlay::Edit(ed) = &a.overlay else {
            panic!()
        };
        assert!(!ed.inserting, "form flipped onto the inserted record");
        a.apply(Command::EditMove(0));
        a.apply(Command::EditBegin);
        if let Overlay::Edit(ed) = &mut a.overlay {
            ed.editing = Some(String::new());
        }
        for c in "twice".chars() {
            a.apply(Command::EditChar(c));
        }
        a.apply(Command::EditCommitField); // ...and Enter again UPDATES it
        let q =
            a.db.query("SELECT count(*) FROM t WHERE b IN ('once','twice')")
                .unwrap();
        assert_eq!(q.rows[0][0], PValue::Int(1), "no duplicate insert");
    }

    #[test]
    fn table_designer_creates_a_real_editable_table() {
        let mut a = app();
        a.apply(Command::OpenCreate(Some("gadgets".into())));
        assert!(matches!(a.overlay, Overlay::Create(_)));
        a.apply(Command::DesignerAdd); // field2
        a.apply(Command::DesignerEditBegin);
        if let Overlay::Create(st) = &mut a.overlay {
            st.editing = Some("label".into());
        }
        a.apply(Command::DesignerCommit);
        a.apply(Command::CreateNull); // label NOT NULL
        a.apply(Command::DesignerAdd); // field3
        a.apply(Command::DesignerCycle); // TEXT → REAL
        a.apply(Command::DesignerEditAlt);
        if let Overlay::Create(st) = &mut a.overlay {
            st.editing = Some("1".into());
        }
        a.apply(Command::DesignerCommit); // default 1
        a.apply(Command::DesignerRun);
        a.sync();
        assert!(matches!(a.overlay, Overlay::None), "designer closed");
        assert!(
            matches!(
                &a.grid,
                Some(Grid { source: GridSource::Table { name, .. }, .. }) if name == "gadgets"
            ),
            "opened BROWSE on the new table"
        );
        let sql =
            a.db.query("SELECT sql FROM sqlite_master WHERE name = 'gadgets'")
                .unwrap();
        let PValue::Text(ddl) = &sql.rows[0][0] else {
            panic!()
        };
        assert!(ddl.contains("\"id\" INTEGER PRIMARY KEY"), "{ddl}");
        assert!(ddl.contains("\"label\" TEXT NOT NULL"), "{ddl}");
        assert!(ddl.contains("\"field3\" REAL DEFAULT 1"), "{ddl}");
        assert!(a.db.has_rowid("gadgets"));
    }

    #[test]
    fn saved_value_stays_on_the_form_after_insert_then_update() {
        // The field-by-field NEW flow: first Enter INSERTS the record
        // (other columns NULL), later Enters UPDATE it. The form must
        // keep showing what was saved — not revert to the stale NULL
        // snapshot taken at insert time (grid right, form wrong).
        let (db, _) = EmbeddedDb::open(":memory:").unwrap();
        db.execute("CREATE TABLE people(id INTEGER PRIMARY KEY, first TEXT, last TEXT)")
            .unwrap();
        let mut a = App::new(Box::new(db), None);
        a.apply(Command::OpenSelected);
        a.sync();
        a.apply(Command::OpenInsert);
        a.apply(Command::EditMove(1)); // skip the auto pk
        a.apply(Command::EditBegin);
        for c in "Mark".chars() {
            a.apply(Command::EditChar(c));
        }
        a.apply(Command::EditCommitField); // advance + INSERT
        a.apply(Command::EditBegin);
        for c in "Cotner".chars() {
            a.apply(Command::EditChar(c));
        }
        a.apply(Command::EditCommitField); // advance + UPDATE
        let Overlay::Edit(ed) = &a.overlay else {
            panic!()
        };
        assert!(ed.inputs.iter().all(|i| i.is_none()), "record is clean");
        let shown: Vec<&PValue> = ed.fields.iter().map(|(_, v)| v).collect();
        assert!(
            shown.iter().any(|v| **v == PValue::Text("Cotner".into())),
            "form reverted a saved value to its stale snapshot: {shown:?}"
        );
    }

    #[test]
    fn typing_replaces_prefilled_values_backspace_edits_them() {
        let mut a = app();
        a.apply(Command::OpenSelected);
        a.sync();
        a.apply(Command::OpenEdit);
        a.apply(Command::EditMove(1)); // b = "row1"
        a.apply(Command::EditBegin); // prefilled with "row1"
        for c in "fresh".chars() {
            a.apply(Command::EditChar(c));
        }
        let Overlay::Edit(ed) = &a.overlay else {
            panic!()
        };
        assert_eq!(
            ed.editing.as_deref(),
            Some("fresh"),
            "first keystroke must REPLACE the prefill, not append"
        );
        // Backspace first → edit mode: prefill retained minus one char.
        a.apply(Command::Back); // cancel buffer
        a.apply(Command::EditBegin);
        a.apply(Command::EditBackspace);
        a.apply(Command::EditChar('X'));
        let Overlay::Edit(ed) = &a.overlay else {
            panic!()
        };
        assert_eq!(ed.editing.as_deref(), Some("rowX"));
        a.apply(Command::Back);
        a.apply(Command::Back);

        // Same contract in the table designer's name editor.
        a.apply(Command::OpenCreate(Some("g".into())));
        a.apply(Command::DesignerAdd); // "field2"
        a.apply(Command::DesignerEditBegin);
        for c in "qty".chars() {
            a.apply(Command::DesignerChar(c));
        }
        a.apply(Command::DesignerCommit);
        let Overlay::Create(st) = &a.overlay else {
            panic!()
        };
        assert_eq!(st.draft.fields[1].name, "qty", "no backspacing required");
    }

    #[test]
    fn paging_accelerates_while_held() {
        let mut a = app();
        a.apply(Command::OpenSelected);
        a.sync();
        a.apply(Command::OpenEdit);
        // 30 rapid presses: streak k gives stride min(1 + k/6, 10) —
        // 6·1 + 6·2 + 6·3 + 6·4 + 6·5 = 90 records covered.
        for _ in 0..30 {
            a.apply(Command::EditPage(1));
        }
        a.sync();
        let Overlay::Edit(ed) = &a.overlay else {
            panic!()
        };
        assert_eq!(ed.row_abs, 90, "held paging must accelerate");
        // A pause resets the streak back to single-stepping.
        a.last_edit_page = Some(std::time::Instant::now() - std::time::Duration::from_millis(400));
        a.apply(Command::EditPage(1));
        a.sync();
        let Overlay::Edit(ed) = &a.overlay else {
            panic!()
        };
        assert_eq!(ed.row_abs, 91, "a pause resets to stride 1");
    }

    #[test]
    fn paint_a_form_and_edit_uses_it() {
        let mut a = app();
        // List designer → F2 → painter; fields auto-place in a column.
        a.apply(Command::OpenForm(Some("t".into())));
        a.apply(Command::DesignerRun);
        let Overlay::Paint(st) = &a.overlay else {
            panic!("painter did not open");
        };
        assert_eq!(st.spec.fields[0].pos, Some((2, 1)), "auto-placed");
        assert_eq!(st.spec.fields[1].pos, Some((2, 3)));

        // Select field b (cursor snaps to its spot), walk to (10, 5),
        // and place it there.
        a.apply(Command::DesignerCycle);
        for _ in 0..8 {
            a.apply(Command::PaintMove { dx: 1, dy: 0 });
        }
        a.apply(Command::PaintMove { dx: 0, dy: 1 });
        a.apply(Command::PaintMove { dx: 0, dy: 1 });
        a.apply(Command::PaintPlace);
        // A title text typed at the cursor — moved OFF the field first
        // (x deletes most-specific-first, and overlap would shadow it).
        a.apply(Command::PaintMove { dx: 10, dy: -5 });
        a.apply(Command::PaintText);
        for c in "ROW ENTRY".chars() {
            a.apply(Command::DesignerChar(c));
        }
        a.apply(Command::DesignerCommit);
        // A box: corner at cursor, far corner after moving.
        a.apply(Command::PaintBox);
        a.apply(Command::PaintMove { dx: 5, dy: 3 });
        a.apply(Command::PaintBox);
        a.apply(Command::DesignerSave);

        // Reload from storage: painted, with everything in place.
        let spec = crate::forms::FormSpec::load(a.db.link(), "t").unwrap();
        assert!(spec.painted());
        assert_eq!(spec.fields[1].pos, Some((10, 5)));
        assert_eq!(spec.texts.len(), 1);
        assert_eq!(spec.texts[0].text, "ROW ENTRY");
        assert_eq!(spec.boxes.len(), 1);
        assert_eq!(spec.boxes[0].w, 6);
        assert_eq!(spec.boxes[0].h, 4);

        // EDIT now carries the painted layout.
        a.apply(Command::Back); // painter → list designer
        a.apply(Command::Back); // close
        a.apply(Command::OpenSelected);
        a.sync();
        a.apply(Command::OpenEdit);
        let Overlay::Edit(ed) = &a.overlay else {
            panic!("edit did not open");
        };
        assert!(ed.painted.is_some(), "EDIT renders the painted form");

        // And 'x' in the painter unplaces the field under the cursor.
        a.apply(Command::Back);
        a.apply(Command::OpenForm(Some("t".into())));
        a.apply(Command::DesignerRun);
        if let Overlay::Paint(st) = &mut a.overlay {
            st.cursor = (10, 5);
        }
        a.apply(Command::PaintDelete);
        let Overlay::Paint(st) = &a.overlay else {
            panic!("painter gone");
        };
        assert_eq!(st.spec.fields[1].pos, None, "field unplaced by x");
    }
}

/// An ALTER-added REAL column edits cleanly through the bus (the CRM
/// demo's balance beat, pinned so a binding regression can't return).
#[test]
fn alter_added_column_edits_cleanly() {
    use crate::db::EmbeddedDb;
    let (db, _) = EmbeddedDb::open(":memory:").unwrap();
    db.execute(
        "CREATE TABLE customers(id INTEGER PRIMARY KEY, name TEXT, city TEXT);
         INSERT INTO customers(name, city) VALUES ('Ada', 'London');
         ALTER TABLE customers ADD COLUMN balance real default 0;",
    )
    .unwrap();
    let mut a = App::new(Box::new(db), None);
    a.apply(Command::OpenSelected);
    a.sync();
    a.apply(Command::OpenEdit);
    a.apply(Command::EditMove(3));
    a.apply(Command::EditBegin);
    for c in "120.5".chars() {
        a.apply(Command::EditChar(c));
    }
    a.apply(Command::EditCommitField);
    a.sync(); // the fresh window arrives async
    assert!(
        !a.status.as_ref().is_some_and(|(_, e)| *e),
        "status: {:?}",
        a.status
    );
    let g = a.grid.as_ref().unwrap();
    assert_eq!(g.row(0).unwrap()[3], PValue::Real(120.5));
}

/// #14: `import <table> <path>` and `export <table|SELECT> <path>` run
/// through the prompt and the real DbLink; empty CSV fields become NULL.
#[test]
fn prompt_import_export_csv() {
    use crate::db::EmbeddedDb;
    let dir = std::env::temp_dir();
    let csv_in = dir.join(format!("phosphor-app-import-{}.csv", std::process::id()));
    let csv_out = dir.join(format!("phosphor-app-export-{}.csv", std::process::id()));
    std::fs::write(&csv_in, "name,city\nAda,London\nGrace,\n").unwrap();

    let (db, _) = EmbeddedDb::open(":memory:").unwrap();
    db.execute("CREATE TABLE people(id INTEGER PRIMARY KEY, name TEXT, city TEXT)")
        .unwrap();
    let mut a = App::new(Box::new(db), None);

    let cmd = format!("import people {}", csv_in.display());
    for c in cmd.chars() {
        a.apply(Command::PromptChar(c));
    }
    a.apply(Command::PromptRun);
    a.sync();
    assert!(
        a.status
            .as_ref()
            .is_some_and(|(m, e)| !*e && m.contains("2 row(s)")),
        "status: {:?}",
        a.status
    );
    let q =
        a.db.query("SELECT name, city FROM people ORDER BY name")
            .unwrap();
    assert_eq!(q.rows[0][1], PValue::Text("London".into()));
    assert_eq!(q.rows[1][1], PValue::Null, "empty CSV field is NULL");

    let cmd = format!(
        "export \"select name from people where name='Ada'\" {}",
        csv_out.display()
    );
    for c in cmd.chars() {
        a.apply(Command::PromptChar(c));
    }
    a.apply(Command::PromptRun);
    a.sync();
    assert!(
        a.status
            .as_ref()
            .is_some_and(|(m, e)| !*e && m.contains("1 row(s)")),
        "status: {:?}",
        a.status
    );
    let out = std::fs::read_to_string(&csv_out).unwrap();
    assert!(out.contains("Ada") && !out.contains("Grace"), "{out:?}");

    let _ = std::fs::remove_file(&csv_in);
    let _ = std::fs::remove_file(&csv_out);
}

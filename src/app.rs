//! App state + the command bus (DESIGN.md, scripting-ready rule 1).
//!
//! Keys become `Command`s; `apply` is the ONLY place state changes. A
//! future script emits the same commands through the same function and
//! inherits every behavior and check for free.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::appsgen::{self, ActionKind, AppDesignState, AppItem, AppMenuState};
use crate::creator::CreateState;
use crate::db::{ColumnInfo, DbLink, PValue, TableInfo};
use crate::forms::{BoxItem, FormSpec, FormState, PaintState, TextItem};
use crate::help::{self, HelpState};
use crate::qbe::{QbeSpec, QbeState};
use crate::report::{self, PagerState, ReportSpec, ReportState};
use crate::theme::{self, Theme};

const OVERSCAN: i64 = 64;
const WIDTH_SAMPLE: usize = 50;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Focus {
    Sidebar,
    Grid,
    Prompt,
}

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
    /// Edited text per field; None = untouched.
    pub inputs: Vec<Option<String>>,
    pub cursor: usize,
    /// Some(buffer) while a field is being typed into.
    pub editing: Option<String>,
}

impl EditState {
    pub fn dirty(&self) -> bool {
        self.inputs.iter().any(Option::is_some)
    }
}

pub enum GridSource {
    Table {
        name: String,
        editable: bool,
    },
    Query {
        truncated: bool,
    },
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
                let mut w = name.len();
                for row in self.cache.iter().take(WIDTH_SAMPLE) {
                    if let Some(v) = row.get(c) {
                        w = w.max(v.render().chars().count());
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
    Focus(Focus),
    Back,
    Help,
    Refresh,
    SidebarMove(i64),
    OpenSelected,
    GridMove { dr: i64, dc: i64 },
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
    PagerScroll(i64),
    PagerWrite,
    OpenForm(Option<String>),
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
    PaintMove { dx: i64, dy: i64 },
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
}

pub struct App {
    pub db: Box<dyn DbLink>,
    /// Set by `--app`: Esc at top level returns to this app's menu.
    pub app_home: Option<String>,
    pub theme: &'static Theme,
    pub focus: Focus,
    pub overlay: Overlay,
    pub tables: Vec<TableInfo>,
    pub sidebar_idx: usize,
    pub grid: Option<Grid>,
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
    /// Select-all semantics for prefilled single-line editors: the
    /// first typed char REPLACES the prefill; Backspace edits it.
    editor_fresh: bool,
    /// Held-key acceleration state for record paging.
    last_edit_page: Option<std::time::Instant>,
    page_streak: u32,
}

impl App {
    pub fn new(db: Box<dyn DbLink>, warning: Option<String>) -> Self {
        let mut app = App {
            db,
            app_home: None,
            theme: &theme::GREEN,
            focus: Focus::Sidebar,
            overlay: Overlay::None,
            tables: Vec::new(),
            sidebar_idx: 0,
            grid: None,
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
            editor_fresh: false,
            last_edit_page: None,
            page_streak: 0,
            last_auto_sample: std::time::Instant::now(),
        };
        app.reload_tables();
        app.health = app.db.health();
        app
    }

    fn say(&mut self, msg: impl Into<String>) {
        self.status = Some((msg.into(), false));
    }

    fn err(&mut self, msg: impl Into<String>) {
        self.status = Some((msg.into(), true));
    }

    /// Internal machinery a user did not create: phosphor's own
    /// catalog, engine shadow tables, and dbhealth's companion views.
    /// Heuristic by naming convention; the i toggle shows everything.
    pub fn is_internal(t: &TableInfo) -> bool {
        let n = t.name.as_str();
        n.starts_with('_')
            || [
                "_chunks", "_meta", "_series", "_blocks", "_terms", "_trace_blocks",
            ]
            .iter()
            .any(|suf| n.ends_with(suf))
            || (t.is_view && ["_report", "_now", "_trends"].iter().any(|s| n.ends_with(s)))
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
    }

    // ── key → command (pure mapping; no state changes here) ──────────

    pub fn map_key(&self, key: KeyEvent) -> Option<Command> {
        use KeyCode::*;
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == Char('q') {
            return Some(Command::Quit);
        }
        if let Overlay::Edit(ed) = &self.overlay {
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
                (Some(_), Char(c)) => Command::EditChar(c),
                (None, Up) => Command::EditMove(-1),
                (None, Down) => Command::EditMove(1),
                (None, PageDown | Right) => Command::EditPage(1),
                (None, PageUp | Left) => Command::EditPage(-1),
                (None, Enter) => Command::EditBegin,
                (None, F(10)) => Command::EditSave,
                (None, Char('s')) if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    Command::EditSave
                }
                (None, F(1)) => Command::Help,
                (None, Esc) => Command::Back,
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
                (None, Up | Char('k')) => Command::DesignerMove(-1),
                (None, Down | Char('j')) => Command::DesignerMove(1),
                (None, Char('n')) => Command::DesignerAdd,
                (None, Char('x')) => Command::DesignerDelete,
                (None, Char('t')) => Command::DesignerCycle,
                (None, Char('p')) => Command::CreatePk,
                (None, Char('r')) => Command::CreateNull,
                (None, Char('u')) => Command::CreateUnique,
                (None, Char('d')) => Command::DesignerEditAlt,
                (None, Char('[')) => Command::DesignerSwap(-1),
                (None, Char(']')) => Command::DesignerSwap(1),
                (None, Enter) => Command::DesignerEditBegin,
                (None, F(2) | F(6)) => Command::DesignerRun,
                (None, F(1)) => Command::Help,
                (None, Esc) => Command::Back,
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
                Tab => Command::Focus(Focus::Prompt),
                _ => return None,
            }),
        }
    }

    // ── the bus ──────────────────────────────────────────────────────

    pub fn apply(&mut self, cmd: Command) {
        let is_delete = matches!(cmd, Command::DeleteRow);
        match cmd {
            Command::Quit => self.quit = true,
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
                    self.sidebar_idx =
                        (self.sidebar_idx as i64 + d).rem_euclid(n) as usize;
                }
            }
            Command::OpenSelected => self.open_selected(),
            Command::GridMove { dr, dc } => self.grid_move(dr, dc),
            Command::GridPage(dir) => self.grid_move(dir * self.visible_rows.max(1), 0),
            Command::GridEdge(end) => {
                if let Some(g) = &mut self.grid {
                    g.cur_col = if end { g.columns.len().saturating_sub(1) } else { 0 };
                }
                self.grid_move(0, 0);
            }
            Command::GridTop => self.grid_jump(0),
            Command::GridBottom => {
                let total = self.grid.as_ref().map_or(0, |g| g.total);
                self.grid_jump(total.saturating_sub(1));
            }
            Command::OpenEdit => self.open_edit(),
            Command::EditMove(d) => {
                if let Overlay::Edit(ed) = &mut self.overlay {
                    let n = ed.fields.len() as i64;
                    if n > 0 {
                        ed.cursor = (ed.cursor as i64 + d).rem_euclid(n) as usize;
                    }
                }
            }
            Command::EditBegin => {
                if let Overlay::Edit(ed) = &mut self.overlay {
                    let current = ed.inputs[ed.cursor].clone().unwrap_or_else(|| {
                        match &ed.fields[ed.cursor].1 {
                            PValue::Null => String::new(),
                            v => v.render(),
                        }
                    });
                    ed.editing = Some(current);
                    self.editor_fresh = true;
                }
            }
            Command::EditChar(c) => {
                let fresh = std::mem::take(&mut self.editor_fresh);
                if let Overlay::Edit(ed) = &mut self.overlay {
                    if let Some(buf) = &mut ed.editing {
                        if fresh {
                            buf.clear(); // first keystroke replaces prefill
                        }
                        buf.push(c);
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
                // empty, the save quietly waits for them.
                self.fold_editing_buffer();
                if let Overlay::Edit(ed) = &mut self.overlay {
                    let n = ed.fields.len();
                    if n > 0 {
                        ed.cursor = (ed.cursor + 1) % n;
                    }
                }
                if self.edit_required_ok() {
                    self.commit_edit();
                }
            }
            Command::EditSave => self.edit_save(),
            Command::PromptChar(c) => {
                // The dot prompt IS the dot: a habitual '.' typed into
                // an empty prompt is a no-op, not a syntax error later.
                if c == '.' && self.prompt.input.is_empty() {
                    return;
                }
                let cur = self.prompt.cursor;
                self.prompt.input.insert(
                    self.prompt
                        .input
                        .char_indices()
                        .nth(cur)
                        .map_or(self.prompt.input.len(), |(i, _)| i),
                    c,
                );
                self.prompt.cursor += 1;
            }
            Command::PromptBackspace => {
                if self.prompt.cursor > 0 {
                    let idx = self
                        .prompt
                        .input
                        .char_indices()
                        .nth(self.prompt.cursor - 1)
                        .map(|(i, _)| i);
                    if let Some(i) = idx {
                        self.prompt.input.remove(i);
                        self.prompt.cursor -= 1;
                    }
                }
            }
            Command::PromptMove(d) => {
                let len = self.prompt.input.chars().count();
                self.prompt.cursor =
                    (self.prompt.cursor as i64 + d).clamp(0, len as i64) as usize;
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
            Command::DesignerAdd => self.designer_add(),
            Command::DesignerDelete => self.designer_delete(),
            Command::DesignerSwap(d) => self.designer_swap(d),
            Command::DesignerEditAlt => self.designer_edit_alt(),
            Command::OpenForm(t) => self.open_form(t),
            Command::OpenApps(name) => self.open_apps(name),
            Command::OpenAppMenu(name) => self.open_app_menu(name),
            Command::OpenInsert => self.open_insert(),
            Command::EditPage(d) => self.edit_page(d),
            Command::DeleteRow => self.delete_row(),
            Command::FindNext => self.find_next(),
            Command::PromptClear => {
                self.prompt.input.clear();
                self.prompt.cursor = 0;
            }
            Command::PromptDeleteWord => {
                let chars: Vec<char> = self.prompt.input.chars().collect();
                let mut i = self.prompt.cursor.min(chars.len());
                while i > 0 && chars[i - 1].is_whitespace() {
                    i -= 1;
                }
                while i > 0 && !chars[i - 1].is_whitespace() {
                    i -= 1;
                }
                let removed: String = chars[..i]
                    .iter()
                    .chain(&chars[self.prompt.cursor.min(chars.len())..])
                    .collect();
                self.prompt.input = removed;
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
        match &mut self.overlay {
            Overlay::Edit(ed) if ed.editing.is_some() => ed.editing = None,
            Overlay::Qbe(st) if st.editing.is_some() => {
                st.editing = None;
                st.naming = false;
            }
            Overlay::Report(st) if st.editing.is_some() => st.editing = None,
            Overlay::Form(st) if st.editing.is_some() => st.editing = None,
            Overlay::Apps(st) if st.editing.is_some() => st.editing = None,
            Overlay::Create(st) if st.editing.is_some() => {
                st.editing = None;
                st.editing_default = false;
            }
            Overlay::Paint(st) if st.editing.is_some() => st.editing = None,
            Overlay::Paint(st) if st.pending_box.is_some() => st.pending_box = None,
            Overlay::Paint(_) => {
                // Painter backs out to the list designer, same spec.
                if let Overlay::Paint(st) =
                    std::mem::replace(&mut self.overlay, Overlay::None)
                {
                    self.overlay = Overlay::Form(FormState {
                        spec: st.spec,
                        cursor: 0,
                        editing: None,
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
                Focus::Grid => self.focus = Focus::Sidebar,
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
        if let Some(Grid {
            source: GridSource::Table { name, .. },
            cur_row,
            cur_col,
            ..
        }) = &self.grid
        {
            let (name, row, col) = (name.clone(), *cur_row, *cur_col);
            self.open_table(&name);
            if let Some(g) = &mut self.grid {
                g.cur_col = col.min(g.columns.len().saturating_sub(1));
            }
            self.grid_jump(row);
        }
        self.health = self.db.health();
        self.say("refreshed");
    }

    // ── phase 3: the dbhealth console ────────────────────────────────

    fn quote_ident(ident: &str) -> String {
        format!("\"{}\"", ident.replace('"', "\"\""))
    }

    /// Find the dbhealth report view and its base vtab, if this
    /// database carries one. Backend-agnostic: plain SQL via DbLink.
    fn find_health_base(&self) -> Option<(String, String)> {
        let q = self
            .db
            .query(
                "SELECT name FROM sqlite_master \
                 WHERE type = 'view' AND name LIKE '%\\_report' ESCAPE '\\' \
                 ORDER BY name LIMIT 1",
            )
            .ok()?;
        let PValue::Text(view) = q.rows.first()?.first()?.clone() else {
            return None;
        };
        let base = view.strip_suffix("_report")?.to_owned();
        Some((view, base))
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

    fn open_health(&mut self) {
        let Some((view, base)) = self.find_health_base() else {
            return self.err(
                "no dbhealth here — needs the timeless extension and \
                 CREATE VIRTUAL TABLE dbhealth USING timeless_health",
            );
        };
        let report = match self.db.query(&format!(
            "SELECT \"check\", status, value, advice FROM {}",
            Self::quote_ident(&view)
        )) {
            Ok(q) => q
                .rows
                .into_iter()
                .map(|r| {
                    [0, 1, 2, 3].map(|i| r.get(i).map(PValue::render).unwrap_or_default())
                })
                .collect(),
            Err(e) => return self.err(e),
        };

        // Sparklines: preferred series first, then whatever else exists.
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
        let available: Vec<String> = self
            .db
            .query(&format!(
                "SELECT DISTINCT name FROM {} ORDER BY name",
                Self::quote_ident(&base)
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
            .unwrap_or_default();
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
            let safe = name.replace('\'', "''");
            if let Ok(q) = self.db.query(&format!(
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
                if let Some(latest) = vals.last().copied() {
                    let rendered = if latest.abs() >= 1_048_576.0 {
                        format!("{:.1} MB", latest / 1_048_576.0)
                    } else if latest.fract() == 0.0 {
                        format!("{latest:.0}")
                    } else {
                        format!("{latest:.3}")
                    };
                    sparks.push((name, vals, rendered));
                }
            }
        }

        self.health = self.db.health();
        self.last_auto_sample = std::time::Instant::now();
        self.overlay = Overlay::Health(HealthView {
            table: base,
            report,
            sparks,
        });
    }

    fn health_sample(&mut self) {
        let Overlay::Health(hv) = &self.overlay else { return };
        let t = Self::quote_ident(&hv.table);
        match self
            .db
            .execute(&format!("INSERT INTO {t}({t}) VALUES ('sample')"))
        {
            Ok((_, elapsed)) => {
                self.last_ms = Some(elapsed.as_secs_f64() * 1000.0);
                self.open_health(); // rebuild report + sparks + dot
                self.say("sampled");
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
        match QbeSpec::new(self.db.as_ref(), &table) {
            Ok(spec) => self.overlay = Overlay::Qbe(QbeState::new(spec)),
            Err(e) => self.err(e),
        }
    }

    fn open_report(&mut self, name: Option<String>) {
        let Some(name) = self.target_table(name) else {
            return self.err("report: no table selected (report <table-or-saved-name>)");
        };
        // A saved report by this name wins; otherwise start from the table.
        let spec = ReportSpec::load(self.db.as_ref(), &name)
            .unwrap_or_else(|| ReportSpec::for_table(&name));
        let columns = self.source_columns(&spec);
        self.overlay = Overlay::Report(ReportState {
            spec,
            cursor: 0,
            editing: None,
            columns,
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
        match report::labels(self.db.as_ref(), &table) {
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
                            Some(i) if i + 1 < st.columns.len() => {
                                Some(st.columns[i + 1].clone())
                            }
                            _ => None,
                        }
                    }
                };
                st.spec.group_by = next;
            }
            _ => {}
        }
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
                    let _ = appsgen::update_item(self.db.as_ref(), &item);
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
                    _ => return, // group_by cycles with Space instead
                });
            }
            Overlay::Form(st) => {
                if let Some(f) = st.spec.fields.get(st.cursor) {
                    st.editing = Some(f.label.clone());
                }
            }
            Overlay::Apps(st) => {
                if let Some(item) = st.items.get(st.cursor) {
                    st.editing_ref = false;
                    st.editing = Some(item.label.clone());
                }
            }
            Overlay::Create(st) => {
                st.editing_default = false;
                st.editing = Some(match st.field_idx() {
                    None => st.draft.table.clone(),
                    Some(i) => st.draft.fields.get(i).map(|f| f.name.clone()).unwrap_or_default(),
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
                    st.editing = Some(item.action_ref.clone());
                }
            }
            Overlay::Create(st) => {
                if let Some(i) = st.field_idx() {
                    if let Some(f) = st.draft.fields.get(i) {
                        st.editing_default = true;
                        st.editing = Some(f.default.clone());
                    }
                }
            }
            _ => {}
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
            if let Some(f) = st
                .spec
                .fields
                .iter_mut()
                .find(|f| f.pos == Some((cx, cy)))
            {
                f.pos = None;
                return;
            }
            if let Some(i) = st
                .spec
                .boxes
                .iter()
                .position(|b| (b.x, b.y) == (cx, cy))
            {
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
                if let Some(buf) = st.editing.take() {
                    match st.cursor {
                        0 => st.spec.title = buf,
                        1 => {
                            st.spec.source = buf;
                            st.spec.group_by = None;
                        }
                        _ => {}
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
                        f.label = buf;
                    }
                }
            }
            Overlay::Apps(st) => {
                if let Some(buf) = st.editing.take() {
                    if let Some(item) = st.items.get_mut(st.cursor) {
                        if st.editing_ref {
                            item.action_ref = buf;
                        } else {
                            item.label = buf;
                        }
                        let item = item.clone();
                        let _ = appsgen::update_item(self.db.as_ref(), &item);
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
                if let Some(buf) = st.editing.take() {
                    match (st.field_idx(), st.editing_default) {
                        (None, _) => st.draft.table = buf.trim().to_owned(),
                        (Some(i), false) => {
                            if let Some(f) = st.draft.fields.get_mut(i) {
                                f.name = buf.trim().to_owned();
                            }
                        }
                        (Some(i), true) => {
                            if let Some(f) = st.draft.fields.get_mut(i) {
                                f.default = buf;
                            }
                        }
                    }
                    st.editing_default = false;
                }
            }
            _ => {}
        }
        if let (Some(name), Overlay::Qbe(st)) = (&save_as, &self.overlay) {
            match st.spec.save(self.db.as_ref(), name) {
                Ok(()) => self.say(format!("saved query {name:?} (run {name})")),
                Err(e) => self.err(e),
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
        match &self.overlay {
            Overlay::Qbe(st) => {
                let sql = st.spec.sql();
                self.overlay = Overlay::None;
                self.run_select(&sql);
            }
            Overlay::Report(st) => {
                let spec = st.spec.clone();
                match report::render(self.db.as_ref(), &spec) {
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
                match draft.create(self.db.as_ref()) {
                    Ok(()) => {
                        self.overlay = Overlay::None;
                        self.reload_tables();
                        let table = draft.table.clone();
                        self.open_table(&table);
                        self.say(format!("created {table:?} — a adds the first record"));
                    }
                    Err(e) => self.err(e),
                }
            }
            Overlay::Form(_) => {
                // F2 in the list designer: enter the painter.
                if let Overlay::Form(st) =
                    std::mem::replace(&mut self.overlay, Overlay::None)
                {
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
            ActionKind::Query => {
                match QbeSpec::saved_sql(self.db.as_ref(), &item.action_ref) {
                    Some(sql) => {
                        self.overlay = Overlay::None;
                        self.run_select(&sql);
                    }
                    None => self.err(format!(
                        "no saved query named {:?} (QBE F6 saves one)",
                        item.action_ref
                    )),
                }
            }
            ActionKind::Report => {
                let spec = ReportSpec::load(self.db.as_ref(), &item.action_ref)
                    .unwrap_or_else(|| ReportSpec::for_table(&item.action_ref));
                match report::render(self.db.as_ref(), &spec) {
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
            ActionKind::Sql => match self.db.execute(&item.action_ref) {
                Ok((n, elapsed)) => {
                    self.last_ms = Some(elapsed.as_secs_f64() * 1000.0);
                    self.reload_tables();
                    self.say(match n {
                        -1 => "ok".to_owned(),
                        n => format!("ok, {n} row(s) affected"),
                    });
                }
                Err(e) => self.err(e),
            },
        }
    }

    fn designer_save(&mut self) {
        match &mut self.overlay {
            Overlay::Qbe(st) => {
                st.naming = true;
                st.editing = Some(String::new());
            }
            Overlay::Report(st) => {
                let spec = st.spec.clone();
                match spec.save(self.db.as_ref()) {
                    Ok(()) => {
                        self.say(format!("saved report {:?} (report {})", spec.name, spec.name))
                    }
                    Err(e) => self.err(e),
                }
            }
            Overlay::Form(st) => {
                let spec = st.spec.clone();
                match spec.save(self.db.as_ref()) {
                    Ok(()) => self.say(format!(
                        "saved form for {:?} — EDIT uses it from now on",
                        spec.table
                    )),
                    Err(e) => self.err(e),
                }
            }
            Overlay::Paint(st) => {
                let spec = st.spec.clone();
                match spec.save(self.db.as_ref()) {
                    Ok(()) => self.say(format!(
                        "saved painted form for {:?} — EDIT renders it now",
                        spec.table
                    )),
                    Err(e) => self.err(e),
                }
            }
            _ => {}
        }
    }

    fn designer_add(&mut self) {
        if let Overlay::Create(st) = &mut self.overlay {
            let i = st.draft.add_field();
            st.cursor = i + 1;
            return;
        }
        if let Overlay::Apps(st) = &self.overlay {
            let app = st.app.clone();
            match appsgen::add_item(self.db.as_ref(), &app, "New item") {
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
                match appsgen::delete_item(self.db.as_ref(), item.id) {
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
                    match appsgen::swap_items(self.db.as_ref(), &a, &b) {
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
        let items = appsgen::items(self.db.as_ref(), app);
        if let Overlay::Apps(st) = &mut self.overlay {
            st.items = items;
            st.cursor = st.cursor.min(st.items.len().saturating_sub(1));
        }
    }

    fn open_form(&mut self, table: Option<String>) {
        let Some(table) = self.target_table(table) else {
            return self.err("form: no table selected (form <table>)");
        };
        let spec = FormSpec::load(self.db.as_ref(), &table)
            .map(Ok)
            .unwrap_or_else(|| FormSpec::new(self.db.as_ref(), &table));
        match spec {
            Ok(spec) => {
                self.overlay = Overlay::Form(FormState {
                    spec,
                    cursor: 0,
                    editing: None,
                })
            }
            Err(e) => self.err(e),
        }
    }

    fn open_apps(&mut self, name: Option<String>) {
        let name = name
            .or_else(|| appsgen::list_apps(self.db.as_ref()).into_iter().next())
            .unwrap_or_else(|| "app".to_owned());
        // Deliberately no ensure here: opening the designer is a READ.
        // The first DesignerAdd creates the app (and its tables).
        let items = appsgen::items(self.db.as_ref(), &name);
        self.overlay = Overlay::Apps(AppDesignState {
            app: name,
            items,
            cursor: 0,
            editing: None,
            editing_ref: false,
        });
    }

    fn open_app_menu(&mut self, name: Option<String>) {
        let Some(name) = name.or_else(|| appsgen::list_apps(self.db.as_ref()).into_iter().next())
        else {
            return self.err("no apps in this database yet — press A to craft one");
        };
        let items = appsgen::items(self.db.as_ref(), &name);
        if items.is_empty() {
            return self.err(format!("app {name:?} has no items yet — A to design"));
        }
        self.overlay = Overlay::AppMenu(AppMenuState {
            app: name,
            items,
            cursor: 0,
        });
    }

    /// Run a SELECT into the query grid (shared by prompt + QBE + apps).
    fn run_select(&mut self, sql: &str) {
        match self.db.query(sql) {
            Ok(q) => {
                self.last_ms = Some(q.elapsed.as_secs_f64() * 1000.0);
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
                self.say(if truncated {
                    format!("{n} rows (capped) — add a WHERE or LIMIT")
                } else {
                    format!("{n} row(s)")
                });
            }
            Err(e) => self.err(e),
        }
    }

    fn open_selected(&mut self) {
        if let Some(t) = self.visible_tables().get(self.sidebar_idx) {
            let name = t.name.clone();
            self.open_table(&name);
        }
    }

    fn open_table(&mut self, name: &str) {
        let start = std::time::Instant::now();
        let cols = match self.db.columns(name) {
            Ok(c) => c,
            Err(e) => return self.err(e),
        };
        let total = match self.db.count(name) {
            Ok(n) => n,
            Err(e) => return self.err(e),
        };
        let editable = self.db.has_rowid(name);
        let mut grid = Grid {
            source: GridSource::Table {
                name: name.to_owned(),
                editable,
            },
            columns: cols.iter().map(|c| c.name.clone()).collect(),
            total,
            cache: Vec::new(),
            cache_start: 0,
            rowids: None,
            cur_row: 0,
            cur_col: 0,
            row_off: 0,
            col_off: 0,
            widths: Vec::new(),
        };
        match self.db.page(name, 0, self.visible_rows + OVERSCAN) {
            Ok(page) => {
                grid.cache = page.rows;
                grid.rowids = page.rowids;
            }
            Err(e) => return self.err(e),
        }
        grid.compute_widths();
        self.last_ms = Some(start.elapsed().as_secs_f64() * 1000.0);
        self.grid = Some(grid);
        self.focus = Focus::Grid;
        self.health = self.db.health();
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
            let used: u16 = g.widths[g.col_off..=g.cur_col]
                .iter()
                .map(|w| w + 1)
                .sum();
            if used <= self.visible_cols_width {
                break;
            }
            g.col_off += 1;
        }
        self.ensure_cache();
    }

    /// Virtualization: keep [row_off-OVERSCAN, row_off+visible+OVERSCAN)
    /// cached for Table sources. Query sources are fully materialized.
    fn ensure_cache(&mut self) {
        let visible = self.visible_rows.max(1);
        let Some(g) = &mut self.grid else { return };
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
        let start = std::time::Instant::now();
        match self.db.page(&name, want_start, limit) {
            Ok(page) => {
                if let Some(g) = &mut self.grid {
                    g.cache = page.rows;
                    g.rowids = page.rowids;
                    g.cache_start = want_start;
                    if g.widths.is_empty() {
                        g.compute_widths();
                    }
                }
                self.last_ms = Some(start.elapsed().as_secs_f64() * 1000.0);
            }
            Err(e) => self.err(e),
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

    /// Build (or rebuild) the EDIT overlay for the record at absolute
    /// grid row `abs` — used by open_edit and by record PAGING.
    fn build_edit_for(&mut self, abs: i64) {
        // Make sure the cache covers the target row, and move the grid
        // cursor with the form so context follows the flip.
        self.grid_jump(abs);
        let Some(g) = &self.grid else { return };
        let GridSource::Table { name, .. } = &g.source else { return };
        let idx = abs - g.cache_start;
        let (Some(row), Some(rowids)) = (g.row(abs), &g.rowids) else {
            return;
        };
        let Some(rowid) = rowids.get(idx as usize).copied() else {
            return;
        };
        let name = name.clone();
        let row: Vec<PValue> = row.clone();
        let cols = match self.db.columns(&name) {
            Ok(c) => c,
            Err(e) => return self.err(e),
        };
        let mut fields: Vec<(ColumnInfo, PValue)> =
            cols.into_iter().zip(row).collect();
        let mut labels: Vec<String> = fields.iter().map(|(c, _)| c.name.clone()).collect();
        let mut required: Vec<bool> = vec![false; fields.len()];

        // A crafted form (phase 5) reorders, relabels, hides, requires;
        // a PAINTED one also brings its 2D layout for rendering.
        let painted = self.apply_crafted_form(&name, &mut fields, &mut labels, &mut required);

        let n = fields.len();
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
            inputs: vec![None; n],
            cursor: keep_cursor,
            editing: None,
        });
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
    fn apply_crafted_form(
        &self,
        table: &str,
        fields: &mut Vec<(ColumnInfo, PValue)>,
        labels: &mut Vec<String>,
        required: &mut Vec<bool>,
    ) -> Option<FormSpec> {
        let spec = FormSpec::load(self.db.as_ref(), table)?;
        let mut ordered = Vec::new();
        let mut new_labels = Vec::new();
        let mut new_required = Vec::new();
        for f in spec.fields.iter().filter(|f| f.include) {
            if let Some(idx) = fields.iter().position(|(c, _)| c.name == f.column) {
                ordered.push(fields[idx].clone());
                new_labels.push(f.label.clone());
                new_required.push(f.required);
            }
        }
        if !ordered.is_empty() {
            *fields = ordered;
            *labels = new_labels;
            *required = new_required;
        }
        spec.painted().then_some(spec)
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
        let cols = match self.db.columns(&name) {
            Ok(c) => c,
            Err(e) => return self.err(e),
        };
        let mut fields: Vec<(ColumnInfo, PValue)> =
            cols.into_iter().map(|c| (c, PValue::Null)).collect();
        let mut labels: Vec<String> = fields.iter().map(|(c, _)| c.name.clone()).collect();
        let mut required: Vec<bool> = vec![false; fields.len()];
        let painted = self.apply_crafted_form(&name, &mut fields, &mut labels, &mut required);
        let n = fields.len();
        self.overlay = Overlay::Edit(EditState {
            table: name,
            inserting: true,
            painted,
            row_abs: 0,
            rowid: 0,
            fields,
            labels,
            required,
            inputs: vec![None; n],
            cursor: 0,
            editing: None,
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
    fn refresh_grid_keep_position(&mut self) {
        if let Some(Grid {
            source: GridSource::Table { name, .. },
            cur_row,
            cur_col,
            ..
        }) = &self.grid
        {
            let (name, row, col) = (name.clone(), *cur_row, *cur_col);
            self.open_table(&name);
            if let Some(g) = &mut self.grid {
                g.cur_col = col.min(g.columns.len().saturating_sub(1));
            }
            self.grid_jump(row);
        }
    }

    /// `find <text>` / 'n': scan forward from the cursor for a row with
    /// any cell containing the needle (case-insensitive). Client-side
    /// scan in pages; capped so a miss on a huge table stays bounded.
    fn find(&mut self, needle: &str) {
        const SCAN_CAP: i64 = 100_000;
        let needle_lc = needle.to_ascii_lowercase();
        let Some(g) = &self.grid else {
            return self.say("find works in a grid");
        };
        let (start, total) = (g.cur_row + 1, g.total);
        let table = match &g.source {
            GridSource::Table { name, .. } => Some(name.clone()),
            GridSource::Query { .. } => None,
        };
        let hit = match table {
            None => {
                let g = self.grid.as_ref().unwrap();
                (start..total).find(|&abs| {
                    g.row(abs).is_some_and(|row| {
                        row.iter()
                            .any(|v| v.render().to_ascii_lowercase().contains(&needle_lc))
                    })
                })
            }
            Some(name) => {
                let mut found = None;
                let mut offset = start;
                let end = total.min(start + SCAN_CAP);
                'scan: while offset < end {
                    let limit = 1024.min(end - offset);
                    match self.db.page(&name, offset, limit) {
                        Ok(page) => {
                            for (i, row) in page.rows.iter().enumerate() {
                                if row.iter().any(|v| {
                                    v.render().to_ascii_lowercase().contains(&needle_lc)
                                }) {
                                    found = Some(offset + i as i64);
                                    break 'scan;
                                }
                            }
                            if page.rows.is_empty() {
                                break;
                            }
                            offset += limit;
                        }
                        Err(e) => return self.err(e),
                    }
                }
                found
            }
        };
        self.last_find = Some(needle.to_owned());
        match hit {
            Some(abs) => {
                self.grid_jump(abs);
                self.focus = Focus::Grid;
                self.say(format!("found at row {}", abs + 1));
            }
            None => self.say(format!("{needle:?} not found below (g for top, n to retry)")),
        }
    }

    fn find_next(&mut self) {
        match self.last_find.clone() {
            Some(n) => self.find(&n),
            None => self.say("no previous find (use: find <text>)"),
        }
    }

    /// Tab at the prompt: complete the last token against table names
    /// and prompt commands.
    fn prompt_complete(&mut self) {
        let input = self.prompt.input.clone();
        let (head, token) = match input.rfind(char::is_whitespace) {
            Some(i) => (&input[..=i], &input[i + 1..]),
            None => ("", input.as_str()),
        };
        if token.is_empty() {
            return;
        }
        let mut candidates: Vec<String> =
            self.tables.iter().map(|t| t.name.clone()).collect();
        candidates.extend(
            [
                "select", "help", "tables", "health", "qbe", "report", "labels",
                "form", "apps", "app", "run", "find", "set theme",
            ]
            .map(str::to_owned),
        );
        let matches: Vec<&String> = candidates
            .iter()
            .filter(|c| c.starts_with(token) && c.as_str() != token)
            .collect();
        match matches.len() {
            0 => self.say(format!("no completion for {token:?}")),
            1 => {
                self.prompt.input = format!("{head}{}", matches[0]);
                self.prompt.cursor = self.prompt.input.chars().count();
            }
            _ => {
                let list: Vec<&str> =
                    matches.iter().take(6).map(|s| s.as_str()).collect();
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
        let Overlay::Edit(ed) = &self.overlay else { return false };
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
        self.fold_editing_buffer();
        // Required validation (crafted forms): the FINAL value of every
        // required field must be non-NULL, edited or not.
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
        let payload = match &self.overlay {
            Overlay::Edit(ed) if !ed.dirty() && !ed.inserting => None,
            Overlay::Edit(ed) => {
                let changes: Vec<(String, PValue)> = ed
                    .fields
                    .iter()
                    .zip(&ed.inputs)
                    .filter_map(|((col, _), input)| {
                        input.as_ref().map(|text| {
                            (col.name.clone(), PValue::parse(text, &col.decl_type))
                        })
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
                .map(|rowid| format!("inserted rowid {rowid}"))
        } else {
            self.db
                .update_row(&table, rowid, &changes)
                .map(|()| format!("saved {n} field(s)"))
        };
        match result {
            Ok(msg) => {
                if inserting {
                    // Total changed: full refresh, then flip the open
                    // form onto the newly inserted record so further
                    // Enters UPDATE it instead of inserting twins.
                    self.refresh_grid_keep_position();
                    let last = self.grid.as_ref().map(|g| g.total - 1).unwrap_or(0);
                    let cursor = match &self.overlay {
                        Overlay::Edit(ed) => ed.cursor,
                        _ => 0,
                    };
                    self.build_edit_for(last.max(0));
                    if let Overlay::Edit(ed) = &mut self.overlay {
                        ed.cursor = cursor.min(ed.fields.len().saturating_sub(1));
                    }
                } else {
                    // Invalidate the cache so the grid shows the new truth.
                    if let Some(g) = &mut self.grid {
                        g.cache.clear();
                        g.cache_start = g.cur_row;
                    }
                    self.ensure_cache();
                    // The record on screen is clean now.
                    if let Overlay::Edit(ed) = &mut self.overlay {
                        for input in ed.inputs.iter_mut() {
                            *input = None;
                        }
                    }
                }
                self.say(msg);
                true
            }
            Err(e) => {
                self.err(e);
                false
            }
        }
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
        self.prompt.cursor = self.prompt.input.chars().count();
    }

    fn prompt_run(&mut self) {
        let line = self.prompt.input.trim().to_owned();
        if line.is_empty() {
            return;
        }
        self.prompt.history.push(line.clone());
        self.prompt.hist_pos = None;
        self.prompt.input.clear();
        self.prompt.cursor = 0;

        // App commands first, SQL otherwise.
        if let Some(rest) = line.strip_prefix("set theme ") {
            return match Theme::by_name(rest.trim()) {
                Some(t) => {
                    self.theme = t;
                    self.say(format!("theme: {}", t.name));
                }
                None => self.err(format!(
                    "unknown theme {:?}; themes: green, amber, paper, blue",
                    rest.trim()
                )),
            };
        }
        if line == "help" {
            return self.open_help();
        }
        if line == "health" {
            return self.open_health();
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
            return match QbeSpec::saved_sql(self.db.as_ref(), name) {
                Some(sql) => self.run_select(&sql),
                None => self.err(format!("no saved query named {name:?}")),
            };
        }
        {
            let toks: Vec<&str> = line.split_whitespace().collect();
            if toks.first().is_some_and(|t| t.eq_ignore_ascii_case("create")) && toks.len() <= 2 {
                let second = toks.get(1).map(|t| t.to_ascii_lowercase());
                let sql_word = matches!(
                    second.as_deref(),
                    Some("table" | "virtual" | "view" | "index" | "trigger"
                        | "temp" | "temporary" | "unique" | "if")
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

        let head = line
            .split_whitespace()
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();
        if matches!(head.as_str(), "select" | "with" | "pragma" | "explain" | "values") {
            self.run_select(&line);
        } else {
            match self.db.execute(&line) {
                Ok((n, elapsed)) => {
                    self.last_ms = Some(elapsed.as_secs_f64() * 1000.0);
                    self.reload_tables();
                    self.health = self.db.health();
                    self.say(match n {
                        -1 => "ok (batch)".to_owned(),
                        n => format!("ok, {n} row(s) affected"),
                    });
                }
                Err(e) => self.err(e),
            }
        }
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
        assert_eq!(a.focus, Focus::Grid);
        let g = a.grid.as_ref().unwrap();
        assert_eq!(g.total, 500);
        a.apply(Command::GridBottom);
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
        let g = a.grid.as_ref().unwrap();
        assert!(matches!(g.source, GridSource::Query { .. }));
        assert_eq!(g.row(0).unwrap()[0], PValue::Int(500));
    }

    #[test]
    fn edit_round_trip_through_the_bus() {
        let mut a = app();
        a.apply(Command::OpenSelected);
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
        assert!(matches!(a.overlay, Overlay::None));
        let g = a.grid.as_ref().unwrap();
        assert_eq!(g.row(0).unwrap()[1], PValue::Text("edited!".into()));
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
        let Overlay::Health(hv) = &a.overlay else {
            panic!("health console did not open");
        };
        assert_eq!(hv.table, "dbhealth");
        assert!(hv.report.len() >= 7, "report rows: {}", hv.report.len());
        assert!(!hv.sparks.is_empty(), "no sparkline series");
        a.apply(Command::HealthSample);
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
        let g = a.grid.as_ref().unwrap();
        assert_eq!(g.total, 1, "exactly row42 matches");
        assert_eq!(g.row(0).unwrap()[1], PValue::Text("row42".into()));
        // And the saved query runs by name from the prompt.
        for c in "run just42".chars() {
            a.apply(Command::PromptChar(c));
        }
        a.apply(Command::PromptRun);
        assert_eq!(a.grid.as_ref().unwrap().total, 1);
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
        assert!(a.status.as_ref().is_some_and(|(m, err)| *err
            && m.contains("required")));
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

    #[test]
    fn find_scans_forward_and_repeats() {
        let mut a = app();
        a.apply(Command::OpenSelected);
        for c in "find row437".chars() {
            a.apply(Command::PromptChar(c));
        }
        a.apply(Command::PromptRun);
        assert_eq!(a.grid.as_ref().unwrap().cur_row, 436);
        // 'n' finds nothing further (unique value) and says so politely.
        a.apply(Command::FindNext);
        assert!(a
            .status
            .as_ref()
            .is_some_and(|(m, _)| m.contains("not found")));
        assert_eq!(a.grid.as_ref().unwrap().cur_row, 436);
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
    fn theme_command() {
        let mut a = app();
        for c in "set theme amber".chars() {
            a.apply(Command::PromptChar(c));
        }
        a.apply(Command::PromptRun);
        assert_eq!(a.theme.name, "amber");
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
        let Overlay::Help(st) = &a.overlay else { panic!() };
        assert_eq!(crate::help::TOPICS[st.topic].key, "reports");
        assert_eq!(st.scroll, 0);
        // Esc closes back toward where the user was.
        a.apply(Command::Back);
        assert!(matches!(a.overlay, Overlay::None));
        // From the prompt, F1 lands on the prompt topic.
        a.apply(Command::Focus(Focus::Prompt));
        a.apply(Command::Help);
        let Overlay::Help(st) = &a.overlay else { panic!() };
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
        assert!(matches!(a.overlay, Overlay::Health(_)), "console stays open");
    }

    #[test]
    fn sidebar_hides_internals_and_seeks_by_letter() {
        let mut a = app();
        a.db
            .execute("CREATE TABLE zeta(x); CREATE TABLE t_chunks(x); CREATE TABLE _phosphor_apps(id INTEGER)")
            .unwrap();
        a.apply(Command::Refresh);
        let names: Vec<String> =
            a.visible_tables().iter().map(|t| t.name.clone()).collect();
        assert!(names.contains(&"t".into()) && names.contains(&"zeta".into()));
        assert!(
            !names.iter().any(|n| n == "t_chunks" || n == "_phosphor_apps"),
            "internals hidden by default: {names:?}"
        );
        // First-letter seek jumps to zeta.
        a.apply(Command::SidebarSeek('z'));
        let vis = a.visible_tables();
        assert_eq!(vis[a.sidebar_idx].name, "zeta");
        // Toggle reveals everything.
        a.apply(Command::ToggleInternals);
        let all: Vec<String> =
            a.visible_tables().iter().map(|t| t.name.clone()).collect();
        assert!(all.iter().any(|n| n == "_phosphor_apps"), "{all:?}");
    }

    #[test]
    fn app_mode_pager_closes_back_to_the_menu() {
        let mut a = app();
        appsgen::ensure_app(a.db.as_ref(), "demo").unwrap();
        appsgen::add_item(a.db.as_ref(), "demo", "Totals").unwrap();
        let mut items = appsgen::items(a.db.as_ref(), "demo");
        items[0].kind = ActionKind::Report;
        items[0].action_ref = "t".into();
        appsgen::update_item(a.db.as_ref(), &items[0]).unwrap();

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
        a.apply(Command::OpenEdit);
        let Overlay::Edit(ed) = &a.overlay else { panic!() };
        assert_eq!(ed.row_abs, 0);
        assert_eq!(ed.fields[1].1, PValue::Text("row1".into()));

        // Page forward: same overlay, next record.
        a.apply(Command::EditPage(1));
        let Overlay::Edit(ed) = &a.overlay else { panic!("form closed") };
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
        let Overlay::Edit(ed) = &a.overlay else { panic!() };
        assert_eq!(ed.row_abs, 2);
        let q = a.db.query("SELECT b FROM t WHERE a = 2").unwrap();
        assert_eq!(q.rows[0][0], PValue::Text("paged-save".into()));

        // Fly to the end: clamped with a message, form stays open.
        for _ in 0..600 {
            a.apply(Command::EditPage(1));
        }
        let Overlay::Edit(ed) = &a.overlay else { panic!() };
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
    fn f10_saves_while_still_typing_in_a_field() {
        let mut a = app();
        a.apply(Command::OpenSelected);
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
        let Overlay::Edit(ed) = &a.overlay else { panic!() };
        assert_eq!(ed.cursor, 0, "advanced (wrapped) to the next field");
        assert!(!ed.dirty(), "record is clean after the Enter-save");
    }

    #[test]
    fn enter_on_new_record_inserts_once_then_updates() {
        let mut a = app();
        a.apply(Command::OpenSelected);
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
        let Overlay::Edit(ed) = &a.overlay else { panic!() };
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
        let q = a.db.query("SELECT count(*) FROM t WHERE b IN ('once','twice')").unwrap();
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
        assert!(matches!(a.overlay, Overlay::None), "designer closed");
        assert!(matches!(
            &a.grid,
            Some(Grid { source: GridSource::Table { name, .. }, .. }) if name == "gadgets"
        ), "opened BROWSE on the new table");
        let sql = a
            .db
            .query("SELECT sql FROM sqlite_master WHERE name = 'gadgets'")
            .unwrap();
        let PValue::Text(ddl) = &sql.rows[0][0] else { panic!() };
        assert!(ddl.contains("\"id\" INTEGER PRIMARY KEY"), "{ddl}");
        assert!(ddl.contains("\"label\" TEXT NOT NULL"), "{ddl}");
        assert!(ddl.contains("\"field3\" REAL DEFAULT 1"), "{ddl}");
        assert!(a.db.has_rowid("gadgets"));
    }

    #[test]
    fn typing_replaces_prefilled_values_backspace_edits_them() {
        let mut a = app();
        a.apply(Command::OpenSelected);
        a.apply(Command::OpenEdit);
        a.apply(Command::EditMove(1)); // b = "row1"
        a.apply(Command::EditBegin);   // prefilled with "row1"
        for c in "fresh".chars() {
            a.apply(Command::EditChar(c));
        }
        let Overlay::Edit(ed) = &a.overlay else { panic!() };
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
        let Overlay::Edit(ed) = &a.overlay else { panic!() };
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
        let Overlay::Create(st) = &a.overlay else { panic!() };
        assert_eq!(st.draft.fields[1].name, "qty", "no backspacing required");
    }

    #[test]
    fn paging_accelerates_while_held() {
        let mut a = app();
        a.apply(Command::OpenSelected);
        a.apply(Command::OpenEdit);
        // 30 rapid presses: streak k gives stride min(1 + k/6, 10) —
        // 6·1 + 6·2 + 6·3 + 6·4 + 6·5 = 90 records covered.
        for _ in 0..30 {
            a.apply(Command::EditPage(1));
        }
        let Overlay::Edit(ed) = &a.overlay else { panic!() };
        assert_eq!(ed.row_abs, 90, "held paging must accelerate");
        // A pause resets the streak back to single-stepping.
        a.last_edit_page = Some(
            std::time::Instant::now() - std::time::Duration::from_millis(400),
        );
        a.apply(Command::EditPage(1));
        let Overlay::Edit(ed) = &a.overlay else { panic!() };
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
        let spec = crate::forms::FormSpec::load(a.db.as_ref(), "t").unwrap();
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

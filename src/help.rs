//! Online help, 1988-style: F1 anywhere opens the topic for the screen
//! you are on, written in English rather than in keybinding shorthand.
//! ←→ walks the topics; the whole manual lives in the binary.

pub struct HelpTopic {
    pub key: &'static str,
    pub title: &'static str,
    pub body: &'static str,
}

pub struct HelpState {
    pub topic: usize,
    pub scroll: u16,
}

pub fn topic_index(key: &str) -> usize {
    TOPICS.iter().position(|t| t.key == key).unwrap_or(0)
}

pub const TOPICS: &[HelpTopic] = &[
    HelpTopic {
        key: "welcome",
        title: "Welcome",
        body: "\
Welcome to phosphor — the green-screen database desktop of 1988,
reborn on a database from 2026.

phosphor is three things in one program:

  · a BROWSER for any SQLite or libSQL database — open a file or
    connect to a server, walk the tables, edit the rows;
  · a BUILDER — design queries, reports, labels, entry forms, and
    menus without writing code (the SQL is shown, never hidden);
  · a RUNTIME — the things you build are stored inside the database
    itself and run as an application for whoever opens it next.

A few habits worth forming on day one:

  · Esc always backs out one level. When in doubt, press it.
  · Editing any prefilled value: just TYPE — the first keystroke
    replaces it. Backspace instead to edit it in place.
  · F1 opens this help on the topic for wherever you are.
    Left and Right arrows move between topics. Esc or F1 returns
    to your previous screen, preserving drafts and unfinished input.
  · F12 shows the full status message and connection details.
    Scroll with arrows or PgUp/PgDn; Esc or F12 returns to your work.
  · The dot prompt (press .) accepts real SQL and short commands.
  · q quits from the top level; Ctrl-Q quits from anywhere.

Everything you build — forms, queries, reports, menus — is saved in
small _phosphor tables inside the database file. Copy the file and
the application travels with it.",
    },
    HelpTopic {
        key: "browse",
        title: "Browsing & editing",
        body: "\
The left panel lists tables (▪) and views (◇). Move with the arrow
keys — or just type a letter to jump to the next table starting
with it — and press Enter to open one in BROWSE, a grid that loads
rows as you scroll, so a million-row table opens instantly.
The table list follows your selection. PgUp/PgDn moves a screenful;
Home/End reaches its first or last table.
phosphor's own machinery (shadow tables, _phosphor catalogs, the
dbhealth views) stays hidden; press i to reveal it. Press C for
the TABLE DESIGNER — define fields as rows while the CREATE
TABLE writes itself underneath. Just TYPE to name the current
field (Enter commits it); the function keys set everything
else: F3 cycles the type, F4 primary key, F5 required, F6
unique, F7 edits the default, F10 sets a FOREIGN KEY (type the
parent table — a bare name points at its primary key, or
table(column) to be exact). F8 (or Ins) inserts a field right
AFTER the cursor — forgot street_name? stand on street_num and
press F8 — F9 (or Del) deletes, [ and ] move a field up and
down; F2 builds the table and opens the empty BROWSE.

Press E (in the sidebar or a BROWSE) for the TABLE EDITOR: the
same structure screen, preloaded with the table's live columns.
Add, rename, or drop columns, or rename the table. The preview
shows the exact changes; F2 applies them together and checks
foreign keys before committing. A failed edit rolls back and
leaves your draft open. Type and constraint changes on existing
columns require a SQL migration: the editor refuses to rebuild
a table from incomplete schema details, protecting constraints,
triggers, indexes, relationships, and table options.
D twice drops the whole table — the same confirm as row delete.
Tables with generated columns need SQL for structure changes;
the designer refuses changes that would lose their expressions.

In the grid:

  · Arrow keys move cell by cell; PgUp/PgDn move a screenful;
    g jumps to the first row and G to the last.
  · Columns fit their data. + and - resize the current column
    (remembered per table), and f freezes the columns up to the
    cursor so they stay put while you scroll right.
  · Enter opens the current row in EDIT — a record form. The form
    is LIVE: land on a field and just start typing (the old value
    is replaced). Enter commits — which also SAVES the record and
    moves to the next field.
    F10/Ctrl-S save and close. Empty input means NULL. A ¶ marks
    the primary key; a * marks a required field. A ƒ marks a computed
    or generated field: it is displayed but cannot be edited.
  · F3 opens the current field in a full-screen NOTE editor — for a
    long comment, memo, or notes field. Enter starts a new line; F6
    folds the text back into the field (the form saves it), Esc
    cancels. It is the same editor that writes Lua scripts.
  · Tab moves to the next field (committing anything typed);
    Shift-Tab moves back. Both skip computed and generated fields.
    Up/Down also lets you inspect read-only fields. Alt-PgUp/PgDn
    pages through fields; Ctrl-Home/End reaches the first or last.
    Long forms scroll to keep the selected field visible.
  · Declared FOREIGN KEYS become child panes under the form —
    open a customer and their orders are right there, refreshed
    live as you page. F4/F5/F6 opens a pane as a filtered BROWSE
    of the child table. On a foreign-key FIELD, F7 opens a picker:
    choose a parent row and its key is written for you. The picker
    shows the match count and loads every parent in pages of 100.
    Press /, type a name, key, or other detail, then Enter to search.
    Search finds literal text across the parent's fields. PgUp/PgDn
    pages through matches; Home/End reaches the first or last match.
    Left/Right shows more columns while keeping the key visible.
    Enter chooses; Esc returns to the form with your draft intact.
    A failed lookup stays open: F5 retries, / changes the search.
  · PgUp/PgDn (or ←/→) in the form flip to the previous/next
    RECORD — and holding the key ACCELERATES, up to ten records a
    stride, so a thousand-row file passes in seconds. Unsaved edits
    SAVE as you page; a failed rule holds the page.
  · a adds a NEW record with the same form. Fields you leave blank
    take the database's own defaults.
  · x deletes the current row — but asks you to press x a second
    time on the same row before anything happens. Any other key
    disarms it.
  · v opens or cycles related tables beside the master grid; H
    stacks the panes. Tab changes which pane has focus. Enter,
    a, and x edit, add, and delete in the focused pane. Related
    forms page only through that parent's records. New rows inherit
    the parent link even if the crafted form hides it. A ↳ marks
    a fixed parent field; use the child's full table to reassign it.
    F10 returns to the related pane and F5 refreshes its records.
    Composite parent links require editing from the child table.
  · Type  find something  at the dot prompt to jump to the next row
    containing that text in any column; n repeats the search.

If a crafted form exists for the table (see Forms), EDIT uses it:
your field order, your labels, your required rules, and — if you
painted one — your screen layout.

Views, query results, and tables without an unambiguous row identity
open read-only. This includes WITHOUT ROWID tables, virtual tables,
and tables that declare all three rowid aliases (rowid, _rowid_, oid).
Ordinary tables can declare one or two aliases and still be edited.",
    },
    HelpTopic {
        key: "prompt",
        title: "The dot prompt",
        body: "\
Press . from anywhere to reach the dot prompt — the fastest way to
talk to a database ever shipped. Type a statement, press Enter.

  · SELECT (or WITH, PRAGMA, EXPLAIN, VALUES) shows results in the
    grid, like any other browse.
  · Anything else — INSERT, UPDATE, CREATE TABLE — executes and
    reports how many rows were affected.

Beyond SQL, the prompt knows a few short commands:

  help              this manual
  tables            back to the table list
  find <text>       search the current grid (n repeats)
  create [name]     the TABLE DESIGNER (raw CREATE TABLE SQL
                    still executes exactly as typed)
  qbe [table]       Query By Example designer
  report [name]     report designer (a table or a saved report)
  labels [table]    mailing labels, three across
  form [table]      form designer (F2 inside it paints)
  apps / app        application designer / run an application
  run <name>        run a query saved from QBE
  import <table> <path>   CSV into a table (header row required)
  export <table|SELECT> <path>  complete CSV out (table or query)
  script <table> <event> <lua>  bind a form lifecycle script
                    (OnValidate, OnSave, OnChange); no lua clears it
  scripts [table]   list bound lifecycle scripts
  edit <table> <event>  multi-line script editor (F6 saves)
  quit / exit       leave phosphor (q and Ctrl-Q work anywhere
                    outside the prompt)
  health            the DBHEALTH console
  advise            index / vacuum advisor (missing FK indexes)
  set theme <name>  green, amber, paper, or blue (remembered)
  set shimmer on|off  CRT scanlines (remembered)
  set boot menu|browser  start at the app menu, or the browser

Comforts: Up/Down walk your history, Tab completes table names and
commands, Ctrl-A/Ctrl-E jump to the ends of the line, Ctrl-U clears
it, Ctrl-W deletes the previous word.",
    },
    HelpTopic {
        key: "qbe",
        title: "Query By Example",
        body: "\
Press Q on a table (or type qbe at the prompt) to open the Query By
Example grid: one line per column, and the SQL phosphor writes from
it displayed at the bottom of the screen at all times. QBE's job is
to teach you SQL while saving you the typing — never to hide it.

For each column you can set three things:

  SHOW    Space toggles whether the column appears in the result.
  SORT    s cycles ascending ▲, descending ▼, or none.
  FILTER  Enter, then type a condition:
            > 100          comparisons pass straight through
            like 'a%'      any SQL operator works
            between 1 and 9
            ada            a bare value means equals — quoted for
                           you unless it is a number

Multiple filters combine with AND.

Two more keys teach the rest of SQL:

  J   cycles a JOIN through the foreign keys that reach this table
      (off, then each related table in turn); the projection is
      qualified so the SQL stays unambiguous.
  g   cycles GROUP BY through the columns; grouping collapses the
      projection to the key plus count(*) AS n.

F2 runs the query into the grid. Esc returns to the same design to
revise or save it. If the query fails, the design stays open.
F6 saves (asks for a name once, then updates that saved query).
F7 Save As creates a separate copy; F8 Rename moves the saved query
and updates menu references. Neither replaces another named query:
to replace one deliberately, reopen it and use F6 Save.
At the prompt, qbe <saved-name> reopens the graphical design, even
after restarting. run <name> executes its saved SQL. A saved query
name takes precedence over a table with the same name.",
    },
    HelpTopic {
        key: "reports",
        title: "Reports & labels",
        body: "\
Press R on a table (or type report at the prompt) to design a
banded report — the kind that produced forty years of business
paperwork: a page header with title and page number, detail lines,
and totals at the bottom.

Three settings, worth exactly three lines on the screen:

  title     Enter to edit. Appears on every page.
  source    a table name, a saved query's SQL, or any SELECT.
  group by  Space cycles through the source's columns; Enter lets
            you type an EXPRESSION instead (substr(city,1,1)).
            Grouping sorts the report, starts a band at each new
            value, and prints subtotals per group.

Columns whose values are all numbers total automatically — per
group and grand. Column widths adapt to the data, and are always
wide enough for their own totals.

F2 previews the report in a pager: arrows and PgUp/PgDn scroll,
w writes the report to a text file, p sends it to a printer
(lp, or $PHOSPHOR_PRINT), Esc returns to the same report designer.
F6 saves (asks for a name once, then updates that saved report).
F7 Save As creates a copy and keeps the original. F8 Rename moves
the saved report and updates menu references. Existing names are
protected: reopen a report and use F6 to replace its saved design.
report <saved-name> reopens it after restarting; application menus
can run it by name. A failed save leaves your design open to retry.

Labels: press L on a table for mailing labels, three across, every
visible column on its own line — Avery energy, zero configuration.

Reports, totals, labels, and CSV exports include every selected row.
The 10,000-row limit applies to interactive SQL results; it does not
limit final output. A LIMIT written in your source SQL still applies.
If the source query fails, no partial report or labels are offered.
CSV and text files replace their destination only after completion;
a failed query or file write leaves an existing output file intact.",
    },
    HelpTopic {
        key: "forms",
        title: "Forms & the painter",
        body: "\
Press F on a table to craft its entry form. The designer lists every
column with four properties:

  SHOW      Space — hidden fields disappear from EDIT entirely.
  REQ       r — required fields refuse to save while empty.
  LABEL     Enter — call the column what humans call it.
  PICTURE   m — a dBASE input mask: 9 = digit, A = letter,
            X/# = alphanumeric, any other character is a literal.
            999-99-9999 formats a social security number as you type
            and refuses a save that does not fit.
  COMPUTED  c — a SQL expression shown read-only on the form
            (qty * price). Calculated per record, never saved; EDIT
            skips over it when you Tab.
  ADD/DEL   n adds a computed field (type its expression right
            away); x removes the selected field from the form.
  order     [ and ] move the field up and down.

F6 saves; from then on EDIT and NEW use your form for that table.
Designer lists follow the selected row as you move. PgUp/PgDn
moves a page; Home/End reaches the first or last row. Long input
also appears above the prompt, with the caret kept in view.

Press F2 for the FORM PAINTER — CREATE SCREEN, reborn. Your fields
appear on a canvas that scrolls to follow your cursor. New fields
extend the canvas instead of overlapping at the bottom. On a
terminal too small for the saved layout, EDIT uses a scrolling list
with the same labels, field order, and validation rules:

  Tab       select the next field (the cursor jumps to it)
  arrows    move the cursor around the canvas
  Space     place the selected field where the cursor stands
  t         type a title or caption at the cursor; Enter places it
  b … b     draw a box: one corner, move, the other corner
  x         delete what is under the cursor (a text, a field's
            placement, or a box by its top-left corner)
  + / -     widen or narrow the selected field's input cell
  F6        save — EDIT now renders your painted screen

Esc from the painter returns to the list designer; nothing is lost
until you leave without saving.",
    },
    HelpTopic {
        key: "apps",
        title: "Applications",
        body: "\
Press A to open the Applications Generator: craft a menu, hand the
database to your team, and it opens as an application.

Each menu item has a label, a kind, and a target:

  browse    opens a table in the grid (with its crafted form)
  query     runs a query saved from QBE, by name
  report    runs a saved report (or a plain table report), by name
  sql       executes a statement — good for one-key housekeeping
  script    a one-line Lua script: query(sql), execute(sql), say(v)

In the designer: n adds an item, Enter edits the label, e edits the
target, c cycles the kind, [ and ] reorder, x deletes, and r
renames the app itself. For a `script` item, E opens the full Lua
editor (see Scripting). All changes save as you go. F2 opens the
live menu to try it; Esc returns to your place in the designer.

The menu itself is pure 1988: arrow keys and Enter, or press the
bright first letter of an item to run it instantly.

To ship it:   phosphor --app yourfile.db
The menu comes up first, and Esc from the top level always returns
to it — the database IS the application. To make that the default
for this database,  set boot menu  at the dot prompt (see Command
line). Since menus, forms, and
reports live in _phosphor tables inside the file, copying the file
deploys the app, and libSQL replication deploys it everywhere.",
    },
    HelpTopic {
        key: "script",
        title: "Scripting (Lua)",
        body: "\
A `script` menu action and form lifecycle events run small Lua
scripts in a sandbox that can touch only the database.

Data globals:
  query(sql)      rows as tables keyed by column name
  query_one(sql)  the first row, or nil
  scalar(sql)     first column of the first row
  execute(sql)    affected rows (-1 for a batch)
  exists(sql)     true when any row comes back
  columns(table)  the table's column names
  quote(s)        a SQL string literal; ident(s) an identifier

Output and helpers:
  say(v) / print(...)  add a line to the result
  trim(s) split(s, sep) join(list, sep) now()
  assert(cond, msg)    stop with msg when cond is false
  json.encode(v) / json.decode(s)

UI effects are QUEUED, then run through the normal command bus —
so read-only mode and every check still apply:
  ui.refresh()       redraw from the database
  ui.browse(table)   open a table in BROWSE
  ui.query(name)     run a query saved from QBE
  ui.report(name)    render a saved report
  ui.form(table)     open the form designer
  ui.prompt()        focus the dot prompt
  ui.quit()          leave phosphor

Bind form events with:
  script <table> OnChange  <lua>   after a field commits
  script <table> OnValidate <lua>  before a save: error(m) blocks,
                                   set(c, v) rewrites a field
  script <table> OnSave    <lua>   after a successful write
Inside them, record is a read/write table of the fields, field is
the field you were on, and is_new says whether it is a new row.
List bindings with  scripts [table]; no lua clears one, and
  edit <table> <event>
opens a multi-line editor (type, Enter for a new line, Tab to
indent, F6 saves, Esc closes without saving). In the
Applications Generator, E opens the same editor on the selected
script menu item.
The editor scrolls vertically and horizontally to follow the caret;
Home/End reaches the start or end of a line. The dot prompt also
keeps the caret visible when a command is longer than the screen.

Sandbox: a 32 MB heap cap and an instruction budget limit runaway
scripts. Lua's math, string, table, and utf8 libraries are available.
File, process, and network APIs, module/script loading, and
coroutines are unavailable. Use the database and ui helpers above.",
    },
    HelpTopic {
        key: "health",
        title: "The DBHEALTH console",
        body: "\
If the database carries timeless-libsql telemetry (a dbhealth
table), press F10 — or type health at the prompt — for the console.

The report lists one row per health check, worst first, each with a
plain-language verdict and one concrete piece of advice: cache hit
ratio, file bloat, WAL size, cache spills, statement memory, growth
rate, and whether sampling is still running at all. Below it,
sparklines chart each metric's recent history straight from the
compressed series.

While the console is open it is LIVE: phosphor takes a sample
every five seconds, so the trends move on their own.

  s   take a sample right now
  r   refresh the console
  Esc back to work — and note that closing the console also stops
      the sampling; the store itself never samples on its own

For continuous history when nobody is watching, run the sample
command on a timer: a one-line cron job calling sqlite3 with the
extension loaded is the classic (the timeless-libsql user guide has
it ready to copy), or have your application sample on its heartbeat.

The status bar keeps a health dot (●) visible at all times: green
is well, amber wants attention, red means read the report.

To give a database dbhealth, load the timeless extension and run:
  CREATE VIRTUAL TABLE dbhealth USING timeless_health;
  INSERT INTO dbhealth(dbhealth) VALUES ('sample');
then sample on a timer or from cron. A year of minute-by-minute
history compresses to about two megabytes.",
    },
    HelpTopic {
        key: "connect",
        title: "Connecting",
        body: "\
phosphor speaks to databases two ways, chosen by the argument:

  phosphor crm.db
      opens the file directly — embedded, no server, microsecond
      queries. The file may be any SQLite or libSQL database.

  phosphor http://host:8880
      connects to a self-hosted sqld server over HTTP. Same
      interface, many users at once — multi-user done right, with
      a real server instead of 1988's file locks.

Environment variables:

  PHOSPHOR_TOKEN   bearer token for authenticated servers
                   (Turso-hosted URLs work with this set).
  PHOSPHOR_EXT     path to libtimeless_ext.so — loads compressed
                   telemetry and dbhealth into embedded databases.
                   Over sqld the server loads the extension instead.

Everything you build is stored in the database itself, so it works
identically over both connections — craft a form on your laptop
against the file, and your team sees it over sqld tomorrow.

Remote transactions stay on one server connection until COMMIT
or ROLLBACK. CSV imports own a transaction and roll back failed
rows, malformed CSV, or a failed commit. They refuse to join a
transaction you already opened. Remote SQL batches stop at the first
error; a failed batch that opened its own transaction rolls it
back. Quoted semicolons, comments, and trigger bodies are kept
intact. Saved labels and scripts preserve quotes and line breaks.

If the server ends a transaction, further writes are skipped.
If a connection failure leaves the outcome unknown, reopen the
database and check the result before retrying. The client does
not reconnect and replay writes automatically.

Startup flags (--app, --readonly, --manual) are in Command line.",
    },
    HelpTopic {
        key: "cli",
        title: "Command line",
        body: "\
Everything the UI does has a startup flag for the shell:

  phosphor [options] [file|url]

  phosphor crm.db              open the file (embedded)
  phosphor http://host:8880    connect to sqld (remote)
  phosphor --app crm.db        boot into the Applications menu
  phosphor --app Billing crm.db   …the app named Billing
  phosphor --app --readonly crm.db   kiosk: browse, never write
  phosphor --manual            print this manual as Markdown
  phosphor --help              usage;  --version  the build

With no argument phosphor opens an in-memory database — a
scratchpad for trying SQL.

--readonly also works without --app. Database writes are blocked
by the backend, including writes hidden in query SQL. Theme,
shimmer, and layout adjustments do not save to the database;
startup preferences cannot be changed. Remote read-only queries
must be SELECTs (including WITH ... SELECT); use SQLite's
pragma_* table functions for metadata. The server parses each
remote read as a SELECT subquery; write APIs are disabled.

The startup destination is a remembered preference:  set boot
menu  opens the app menu on launch,  set boot browser  the table
list (the default). A database that carries an app but no
preference shows a hint pointing at A, so the menu is never a
secret.",
    },
    HelpTopic {
        key: "keys",
        title: "Key reference",
        body: "\
Everywhere
  F1 help · Esc back out · Ctrl-Q quit · . dot prompt
  Tab cycle focus · F10 dbhealth console

Table list
  ↑↓ move · letters seek · i internals · Enter browse
  PgUp/PgDn page · Home/End first/last table
  C table designer · E table editor · r refresh · q quit

BROWSE grid
  ↑↓←→ / hjkl move · PgUp PgDn page · g G first/last row
  Home End first/last column · Enter edit row · a add row
  x (twice) delete row · n find next · F5 refresh
  + / - column width · f freeze at cursor
  v split view · H stack split · Q qbe · R report · L labels
  F form · A applications · E table editor

EDIT / NEW record
  ↑↓/Tab field · PgUp/PgDn (or ←→) previous/next record
  Alt-PgUp/PgDn page fields · Ctrl-Home/End first/last field
  Enter edit value · Enter again commit + SAVE + next field
  F3 full-screen note editor · F7 pick a foreign-key value
  F4/F5/F6 child panes · F10 / Ctrl-S save and close
  Esc cancel value, then close

Foreign-key picker
  / search name, key, or details · Enter search/choose · Esc back
  ↑↓ move · PgUp/PgDn 100 records · Home/End first/last match
  ←→ more columns · F5 retry/refresh · count includes all matches

Designers
  PgUp/PgDn page · Home/End first/last row (when not typing)
  form: Space show · n add · x del · r required · m mask · c computed
        Enter label · [ ] order · F2 painter · F6 save
  qbe:  Space show · Enter filter · s sort · J join · g group
  table: type names · F3 type · F4 pk · F5 not-null · F6 unique
         F7 default · F8 add · F9 delete · [ ] move · F2 apply
  editor: same keys; D D drops the table (E opens it)
  report: Enter edit · Space cycle group · F2 preview · F6 save-as
  apps: n new · Enter label · e target · r name · E script · F2 run

Dot prompt
  Enter run · ↑↓ history · Tab complete
  Ctrl-A/E line ends · Ctrl-U clear · Ctrl-W delete word
  import/export CSV · run <saved query> · set theme/shimmer/boot

Form painter
  Tab field · arrows cursor · Space place · t text · b box
  x delete · +/- width · F6 save

Pager (reports, labels)
  ↑↓ PgUp PgDn scroll · g G ends · w write file · p print

Help
  ←→ topics · ↑↓ PgUp PgDn scroll · Home/End top/bottom · Esc close
  Text wraps to the available width.",
    },
];

/// Render the whole in-app manual as markdown — `phosphor --manual`.
/// The web manual is generated FROM the binary, so the two can never
/// disagree (docs/MANUAL.md is this function's output, committed).
pub fn manual_markdown() -> String {
    let mut out = String::from(
        "# The phosphor manual\n\n\
         *This file is generated from the in-app help (`phosphor --manual`).\n\
         Press F1 inside phosphor for the same manual, opened to the topic\n\
         for whatever screen you are on. Do not edit by hand — edit\n\
         `src/help.rs` and regenerate.*\n",
    );
    for t in TOPICS {
        out.push_str(&format!("\n## {}\n\n```text\n{}\n```\n", t.title, t.body));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manual_covers_every_topic() {
        let md = manual_markdown();
        for t in TOPICS {
            assert!(md.contains(&format!("## {}", t.title)), "{} missing", t.key);
        }
        assert!(md.contains("generated from the in-app help"));
    }

    /// #24: `docs/MANUAL.md` is generated output; it must never drift
    /// from the in-app help. Regenerate with:
    /// `cargo run --bin phosphor -- --manual > docs/MANUAL.md`
    #[test]
    fn committed_manual_is_in_sync() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/MANUAL.md");
        let committed = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        assert_eq!(
            manual_markdown(),
            committed,
            "docs/MANUAL.md is stale — regenerate it from src/help.rs"
        );
    }

    #[test]
    fn topics_resolve_and_have_prose() {
        assert!(TOPICS.len() >= 9);
        for t in TOPICS {
            assert!(!t.body.is_empty(), "{} has no body", t.key);
            assert!(
                t.body.lines().all(|l| l.chars().count() <= 70),
                "{} has a line wider than the help pane",
                t.key
            );
        }
        assert_eq!(topic_index("qbe"), 3);
        assert_eq!(topic_index("nonsense"), 0, "unknown keys land on welcome");
    }
}

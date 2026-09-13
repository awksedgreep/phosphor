# Changelog

## Unreleased

### Added
- **CSV import/export** at the dot prompt (#14): `import <table> <path>`
  (headered, case-insensitive column match, empty → NULL, transactional)
  and `export <table|SELECT> <path>` (proper CSV quoting). New `csv_io`
  module; works embedded or over sqld.
- **Printing** from the pager (#13): `p` writes the report/labels file and
  pipes it to `$PHOSPHOR_PRINT` (default `lp`, then `lpr`); a missing
  printer reports cleanly and leaves the text file for manual printing.
- **Report `GROUP BY` expressions** (#17): the report designer's group-by
  cell accepts any SQL expression (`substr(city,1,1)`), not just a column
  name, via a synthetic ordering column stripped before layout.
- **QBE joins + GROUP BY** (#16): `J` cycles FK-driven JOINs (both
  directions), qualifying the projection; `g` cycles GROUP BY, collapsing
  to `key, count(*) AS n`. The live SQL preview shows both.
- **PICTURE masks** (#15, first slice): form designer `m` sets a dBASE
  input mask (`999-99-9999`); the EDIT buffer formats as you type and a
  save that doesn't fit is refused.
- **Foreign-key pickers** (#15, second slice): `F7` on a declared FK
  field pops a parent-row picker; Enter writes the chosen key into the
  field — dBASE value lookup, no manual id typing.
- **Computed form fields** (#15, third slice): form designer `c` sets a
  SQL expression (`qty * price`) rendered read-only (`ƒ`); it is
  calculated per record in one query and never written back. `n` adds a
  new computed field (its expression editor opens immediately) and `x`
  removes the selected field — computed fields are authorable from the
  UI now, not just hand-written JSON.
- **Persistent appearance** (#22): `set theme` and `set shimmer on|off`
  are remembered in `_phosphor_prefs` across sessions; shimmer dims every
  other screen row (off by default).
- **Manual drift guard** (#24): a test and a CI step assert that
  `docs/MANUAL.md` matches `phosphor --manual`.
- **Perf budgets in CI** (#23): release-only tests assert embedded page
  and worker round-trip stay under budget (measured ~36µs / ~17µs).
- **Key reference** (#25): the F1 key topic now covers the table editor,
  split view, designer keys (mask/join/group), and CSV/print commands.
- **Index & vacuum advisor** (#19): `advise` lists foreign-key columns
  without an index (with the exact `CREATE INDEX`), plus a VACUUM nudge
  from dbhealth's bloat check.
- **Split orientation** (#18): `H` stacks the master above the detail
  pane (default stays side-by-side); the choice is remembered.
- **Column resize & freeze** (the last `DESIGN.md` BROWSE goals): `+`/`-`
  resize the current column — manual widths override auto-fit and persist
  per table — and `f` freezes the columns up to the cursor so they stay
  put while scrolling right (also persisted). Stored in `_phosphor_prefs`
  as `width:<table>` / `freeze:<table>`; a new `columns` UI reel shows it.
- **Drop table from the UI**: `D` in the TABLE EDITOR drops the whole
  table on the second press — the row-delete contract, extended to
  tables. The evolution demo (CRM ch03) now drives the TABLE EDITOR for
  add/drop column and the confirmed drop, and a new `tableeditor` reel
  covers it (19 asserted reels).
- **Live grid after prompt writes**: an INSERT/UPDATE/DELETE at the dot
  prompt refetches the browsed grid in place (cursor kept), and DDL
  reopens the table in view (its columns may have changed) — the pane no
  longer shows rows the write just invalidated. `import` and
  script-driven SQL refresh the same way.
- **Boot destination**: `set boot menu|browser` recalls where a session on
  a database-with-apps should land. When an app is present and no
  preference is set, the status line points at `A` so the menu isn't a
  secret.
- **Note editor**: `F3` in EDIT/NEW opens the current field in the same
  full-screen editor that writes Lua scripts — for long comments, memos,
  and notes. Enter starts a new line; F6 folds the text back into the
  field (the form's own F10 writes the row); Esc cancels. The one-line
  field shows folded newlines as `␤`, and Enter on such a value reopens
  the note editor. New `memo` UI reel (20 asserted reels).

- **Read-only kiosk + app version** (#21): `--app --readonly` refuses
  every write centrally; `_phosphor_apps.version` migrates in place and
  shows in the menu title when above 1.
- **Scripting hook, slice 1** (#12): a new `script` menu action kind runs
  a one-line Lua script (vendored `mlua`/Lua 5.4). The sandbox exposes
  `query(sql)`, `execute(sql)`, and `say(v)` over the same `DbLink`, with
  a 32 MB heap cap and an instruction budget that aborts runaway loops.
- **Scripting hook, slice 2** (#12): form lifecycle scripts. `script
  <table> <event> <lua>` binds Lua to `OnValidate` / `OnSave` / `OnChange`
  in `_phosphor_scripts`, and `scripts [table]` lists them. During EDIT
  the script sees a read/write `record` table, `field`, `is_new`, plus
  `get`/`set`/`error`; `OnValidate` can block a save and rewrite fields,
  `OnChange` runs when a field is committed, `OnSave` runs after the
  write.
- **Scripting hook, slice 3** (#12): effects as values (rule 5) — a
  sandboxed `ui` table queues `ui.refresh()`, `ui.browse(t)`,
  `ui.query(name)`, `ui.report(name)`, `ui.form(t)`, `ui.prompt()`, and
  `ui.quit()`. Menu-script effects are dispatched through the same
  command bus a keystroke uses (rule 1), so readonly and every guard
  apply. New `script` F1 topic documents the whole surface.
- **Scripting hook, slice 4** (#12): the multi-line Lua editor.
  `edit <table> <event>` opens a full-screen editor — line numbers,
  inverse caret, Enter newline, Tab indent, Backspace that joins lines,
  F6 save, Esc close. The `scripts` pager now names the exact `edit`
  command per binding.
- **Scripting hook, slice 5** (#12): the editor also edits **menu-item
  scripts**. In the Applications Generator, `E` opens the selected
  `script` item full-screen; saving returns to the designer, and the
  item list previews multi-line targets with `⏎`. `scripts` lists
  menu-item scripts too.
- **Scripting hook, slice 6** (#12): a richer host API. Alongside
  `query`/`execute`/`say`, scripts get `query_one`, `scalar`, `exists`,
  `columns`, `quote`, `ident`, `print`, `trim`, `split`, `join`, `now`,
  `assert`, and `json.encode`/`json.decode` — the sandbox still reaches
  only the database.

### Fixed
- **timeless #56**: the TABLE EDITOR now drops/renames columns on a
  database that carries the timeless extension (column DROP is back in
  the `tableeditor` reel). Upstream `dbhealth` registered its metric
  modules but not the `timeless_series` TVF that its own
  `timeless_<table>_series` view selects from; SQLite validates every
  view on `ALTER`, so every schema edit failed with
  `no such table: main.timeless_series`. Fixed in timeless-libsql
  `df76dc4`; the extension must be loaded (`PHOSPHOR_EXT`) because the
  view needs the TVF to resolve.
- A cached window statement (`open_window`) snapshotted its column count
  before the first step, so an `ALTER TABLE ... DROP COLUMN` followed by
  a refill read past the narrowed row and failed with
  `Invalid column index`. The width is now read live per row.
- A long status message (startup hint, import error) no longer shoves the
  row position, latency, and health dot off the status line, and an async
  table reopen no longer wipes an `applied N change(s)` message.
- **#11**: Enter on an untouched NEW form no longer INSERTs an all-NULL
  placeholder row; it just advances. F10/Ctrl-S still inserts a
  defaults-only row when asked.
- **#10**: dropping the browsed table closes the zombie grid (stale
  title/rows, quiet "no such table" refills) and returns to the sidebar.
- **#8**: the report designer's F6 now prompts for a name (prefilled)
  instead of saving instantly under the table name; renaming retires the
  old `_phosphor_reports` row, so a source can feed several reports.
- **#7**: the Applications Generator can name/rename the app with `r`
  (items link by id, so they survive); the menu title follows.
- `import csv <table>` / `export csv <source>` mistook a table whose name
  starts with `csv` for the optional `csv` keyword (`csvtest` → `test`;
  caught by the new `data` UI reel).
- BROWSE column widths were computed while a table was still empty and
  then frozen at the header minimum (4), so a table created and filled in
  one session showed `Gra…`/`Lon…` (caught on film: CRM `01-customers`).
  Widths now grow to fit as the window fills, and never shrink mid-session
  (`Grid::grow_widths`).
- The `builders` demo no longer shows the incoherent painter beat (the
  dedicated, asserted `forms` reel owns the painter); new asserted reels
  cover scripting, CSV + advisor, QBE joins/group, PICTURE/F7 pickers,
  and the read-only kiosk.
- Existing reels now also exercise report save-as (rename), app rename
  (`r`), computed-field authoring (`n`), and split `H`.
- The UI-test terminal emulator carries an escape sequence split across
  pty reads instead of leaking its raw parameter bytes onto the grid
  (which made a correct advisor line unfindable).
- `ui::draw` duplicated split-view layout/hit-rect computation (dead block removed).
- TABLE EDITOR `ALTER TABLE RENAME` + `DROP/ADD COLUMN` targeting the old name after a table rename (now targets the live name); column renames already handled.
- `DbHandle::call` buffering `Duration::ZERO` for async responses arriving during a sync call (now preserves worker-measured latency).
- `has_rowid` negative-cache poisoning on transient errors (now only caches definitive `no such column` failures, both embedded and remote).
- `EditCommitField` double `PValue::parse` per required field (now single quiet check + `commit_edit_inner(skip_required)`).

### Chore
- `cargo fmt` across the workspace (was ~2.8k lines of drift) and CI now runs `cargo fmt --check`.
- Demo recordings (`crm.py`, `scenarios.py`) run with `cwd` in a scratch
  directory, so the CRM/scenario runs no longer deposit `report_*.txt`
  in the repo root (the stray tracked files are removed; `.gitignore`
  covers new ones). Main and UI demo GIFs refreshed.

## 0.1.0 — 2026-08-04

The first tagged release: all five founding phases plus a season of
polish, every feature landed with tests and on-film verification.

### The desktop
- **BROWSE**: virtualized grid (millions of rows), first-letter seek in
  the sidebar, internals hidden behind a toggle, read-only views,
  `find <text>` + `n`, insert (`a`), double-`x` delete.
- **EDIT / NEW**: record forms — auto-generated or crafted — with
  required-field enforcement, typed parsing, and **record paging**:
  PgUp/PgDn (or ←→) flip records with held-key acceleration (up to 10
  records/stride, ~250 rec/s at typical autorepeat); dirty edits commit
  as you page.
- **The dot prompt**: real SQL into the grid, app commands, history,
  Tab completion, Ctrl-A/E/U/W, four themes (green/amber/paper/blue).

### The builders
- **Query By Example** with always-visible (wrapping) generated SQL;
  saved queries replayable via `run <name>`.
- **Banded reports** (group bands, subtotals, identifier-safe totals),
  mailing labels, file output.
- **Forms**: list designer + the 2D **FORM PAINTER** (fields at x/y,
  texts, boxes) rendered by EDIT/NEW from then on.
- **The Applications Generator**: menus of browses/queries/reports/SQL;
  `phosphor --app db` boots the menu; apps live in `_phosphor_*` tables
  and travel with the file.

### The platform
- Two backends behind one trait: embedded SQLite/libSQL files and
  self-hosted sqld over Hrana HTTP (`PHOSPHOR_TOKEN` for auth).
- **DBHEALTH console** (F10): plain-language report + sparklines, LIVE
  sampling while open (timeless-libsql's dbhealth extension).
- Context-sensitive **F1 manual** in the binary; `--manual` renders it
  to markdown (docs/MANUAL.md is generated output); `--help`/`--version`.
- Docs: Build-a-CRM tutorial, UI tour — every GIF produced by the
  on-screen-asserting UI test sweep (`tools/demo/uitest.py`), which
  renders demos only from green runs.
- `phosphor-seed` (optional `seed` feature): fake-data generator for
  large test databases.

42 unit tests through the command bus, ten UI reels asserting the
visible screen, clippy-clean, CI on every push.

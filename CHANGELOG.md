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
  calculated per record in one query and never written back.
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
  write. Still open: scripts emitting `Command`s through the bus.

### Fixed
- `ui::draw` duplicated split-view layout/hit-rect computation (dead block removed).
- TABLE EDITOR `ALTER TABLE RENAME` + `DROP/ADD COLUMN` targeting the old name after a table rename (now targets the live name); column renames already handled.
- `DbHandle::call` buffering `Duration::ZERO` for async responses arriving during a sync call (now preserves worker-measured latency).
- `has_rowid` negative-cache poisoning on transient errors (now only caches definitive `no such column` failures, both embedded and remote).
- `EditCommitField` double `PValue::parse` per required field (now single quiet check + `commit_edit_inner(skip_required)`).

### Chore
- `cargo fmt` across the workspace (was ~2.8k lines of drift) and CI now runs `cargo fmt --check`.

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

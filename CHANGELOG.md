# Changelog

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

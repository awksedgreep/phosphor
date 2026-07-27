# phosphor — design

**Status: founding document, 2026-07-27.** Written before the code, on
purpose; the decisions below are commitments, the details are drafts.

## What we are actually building

A terminal application platform in the lineage of the late-80s database
desktops (dBASE IV, Paradox, FoxPro — named here historically; see the
trademark note at the end): one binary that is simultaneously a database
browser, a form/report/menu *builder*, and the *runtime* for the
applications users build with it — backed by SQLite/libSQL with the
[timeless-libsql](https://github.com/awksedgreep/timeless-libsql)
extension for compressed telemetry and self-monitoring (`dbhealth`).

The test of success is the CRM scenario: a person who is not a programmer
sits down with phosphor, creates tables, paints an entry form, defines two
reports and a menu that ties them together — and their team then *uses* that
application daily, over sqld, at terminal speed.

## The revival map

What we bring back, what it becomes. (The left column names dBASE IV
features by their historical names for comparison — nominative use; no
affiliation or compatibility is implied or intended.)

| the 1988 feature | phosphor (2026) | notes |
|---|---|---|
| The dot prompt | SQL + app-command REPL with history & completion | the identity feature; always one keystroke away |
| Control Center / ASSIST | The home screen: panels for Data, Queries, Forms, Reports, Apps, Admin | the ASCII mock in the README |
| BROWSE | Grid view/edit of any table or query, virtualized for millions of rows | column resize, freeze, seek-as-you-type |
| EDIT | Single-record form view, auto-generated when no painted form exists | one key flips BROWSE ↔ EDIT |
| CREATE SCREEN (`.scr`/`.fmt`) | Full-screen form painter: fields, labels, pickers, checkboxes, validation rules, field order | stored in the db (see "Apps live in the database") |
| `@ SAY/GET` + PICTURE clauses | Field masks & validation (`999-99-9999`, ranges, required, lookup-into-table) | declarative, in the form definition |
| CREATE REPORT (`.frm`) | Banded report writer: page/group headers & footers, detail band, computed totals; output to screen pager, text file, or printer | groups from `GROUP BY`-able expressions |
| CREATE LABEL (`.lbl`) | Label writer (Avery presets and custom geometry) | yes, really — it's 30 lines of layout math and people still print labels |
| CREATE QUERY / QBE (`.qbe`) | Query-by-example grid that *shows the generated SQL* | teaches SQL instead of hiding it |
| Applications Generator (`.app`) | Menu designer: named menus of actions (open form, run query/report, run SQL, submenu) | the "craft your own CRM" feature |
| `SET RELATION` | Master-detail linked browses driven by foreign keys (declared or ad-hoc) | orders under the selected customer, live |
| Catalogs (`.cat`) | An "app" = named collection of tables/forms/queries/reports/menus | one db can hold several apps |
| Memo fields (`.dbt`) | TEXT columns with a full-screen editor pop-over | plus JSON columns with a tree editor |
| Indexes (`.ndx`/`.mdx`) | Real SQL indexes + an index advisor fed by `dbhealth` | "this browse full-scans; create index?" |
| `PROTECT` (users/passwords) | Delegated to deployment: file permissions (embedded) or sqld auth (network) | phosphor is not an auth system |
| Function keys, status bar | F1 help, F2 data, F10 menu, Esc backs out, status bar with db/latency/health dot | keyboard-first, mouse tolerated |
| `SET` commands | A `set` command namespace at the dot prompt (`set theme amber`) | persisted per-user |
| dBASE language (`DO WHILE`, `.prg`) | **Not revived in v1.** SQL + the menu/action layer covers the 90% case | a scripting hook (Lua?) is a later, separate decision |

Deliberate omissions: the dBASE-style language interpreter (see above),
`.dbf` file compatibility — **phosphor reads and writes SQLite/libSQL
databases, period** (CSV import/export covers migration), and multi-user
file locking (that's sqld's job now — this is the part 1988 got wrong and
we don't have to).

## Throwforward: what 1988 couldn't do

- **`dbhealth` console (Admin panel).** `SELECT * FROM dbhealth_report`
  rendered as the system screen: status lights, advice, trends sparklines.
  The status bar carries a green/amber/red health dot at all times. An
  index advisor and vacuum advisor grow out of the same data.
- **Telemetry as first-class panels.** timeless logs tail (level-colored,
  filter-as-you-type), metric sparklines, trace waterfalls — the same
  terminal that runs your CRM watches your infrastructure.
- **Apps replicate with their data.** Because definitions are rows (below),
  libSQL replication deploys the application to every replica. Mail
  someone the file, you mailed them the program.
- **Themes.** P1 green (default), P3 amber, paperwhite, IBM 3278 blue.
  CRT affectations (scanline shimmer) optional and off by default.
- **Speed as a feature.** Budgets, enforced in CI: < 16 ms per frame,
  < 1 ms embedded query round-trip for BROWSE paging, startup < 100 ms.
  The whole point is that it *feels* like 1988 hardware never got slow.

## Apps live in the database

All designer output is stored in `_phosphor_*` tables inside the target
database (namespaced, created on first save, ignorable by other tools):

```sql
_phosphor_apps    (id, name, description, menu_root)
_phosphor_menus   (id, app_id, title, position)
_phosphor_items   (id, menu_id, label, action_kind, action_ref, hotkey, seq)
_phosphor_forms   (id, name, table_ref, layout_json, version)
_phosphor_queries (id, name, qbe_json, sql_text, version)
_phosphor_reports (id, name, source_ref, bands_json, version)
_phosphor_prefs   (user, key, value)
```

`layout_json`/`bands_json` are versioned documents; the schema is the
contract and migrations are explicit. Everything a user builds is
`SELECT`-able, diffable, and replicable. (Familiar trick: it's the shadow-
table pattern from timeless-libsql, one layer up.)

## Architecture

```
┌────────────────────────────────────────────────────────┐
│ UI runtime (ratatui): screens, widgets, keymaps, themes│
│   home │ browse/edit │ painters │ report pager │ admin │
├────────────────────────────────────────────────────────┤
│ App model: forms/menus/reports/queries (rows ⇄ structs)│
├────────────────────────────────────────────────────────┤
│ DbLink trait: query(sql, params) → columns + rows      │
│               execute / batch / stream, schema introsp.│
├──────────────────────────┬─────────────────────────────┤
│ Embedded backend         │ Remote backend              │
│ rusqlite, opens the file │ Hrana over HTTP (/v3/       │
│ + loads libtimeless_ext  │ pipeline) to self-hosted    │
│ (fastest; single-process)│ sqld (multi-user; the       │
│                          │ extension loads server-side)│
└──────────────────────────┴─────────────────────────────┘
```

- **Two backends, one trait, decided at "open".** `phosphor crm.db` vs
  `phosphor http://host:8880`. The UI is backend-agnostic; capabilities
  (timeless modules present? dbhealth table exists?) are *detected*, and
  panels light up accordingly. Plain SQLite files with no extension get a
  first-class experience too — phosphor must be worth using on any db.
- **Embedded** is the speed king and the reason BROWSE can feel telepathic:
  rusqlite prepared statements, keyset pagination, no serialization.
- **Remote** reuses the verified Hrana pipeline work from the
  timeless-libsql docs (typed JSON values, integers as strings, blobs
  base64). One pipeline per user action; batches where transactional.
- **No async runtime in v1.** A worker thread owns the DbLink; the UI
  thread owns the terminal; they speak over channels. Boring and fast.

## Phasing

| phase | scope | proves |
|---|---|---|
| **0 — splash** (now) | repo, manifesto, compiling ratatui skeleton | the name renders in green |
| **1 — browser** | open embedded db; schema sidebar; BROWSE (virtualized grid) + EDIT (auto form); dot prompt running real SQL with history; themes | phosphor is already the nicest way to poke a SQLite file |
| **2 — remote** | DbLink over Hrana/sqld; capability detection; same UI | multi-user story works |
| **3 — dbhealth console** | Admin panel: report, trends, sparklines; status-bar health dot | "db intelligence" is real |
| **4 — QBE + reports** | query grid → SQL; banded report writer + pager; labels | the paperwork engine |
| **5 — forms + apps** | form painter; menu designer; app runtime mode (`phosphor --app crm.db`) | the CRM scenario, end to end |

Each phase ships usable. The order is deliberate: browsing pays rent
immediately, and the builder features land on a UI that already feels right.

## Open questions

1. Name: `phosphor` chosen for the founding commit (the glow of a P1
   CRT). GitHub renames redirect, so this is reversible cheaply.
2. Scripting hook (Lua? Rhai? none?) — **tabled (author decision,
   2026-07-27)**. Nothing in phases 1–5 depends on the answer: the
   menu/action layer is declarative, and its `action_kind` enum is the
   natural extension point if a scripting action is ever added. Revisit
   only when a real user hits the declarative layer's ceiling.
3. Printing path for reports/labels (direct to `lp`? text file only?) —
   decide in phase 4 with real users' printers in mind.
4. Whether the embedded backend bundles SQLite (static, with extension
   statically linked) or uses the system library + `.so` — bundling is
   likelier (one binary, no "not authorized" macOS surprises).

---

*Trademark note: dBASE® is a trademark of dBase, LLC; Paradox and FoxPro
belong to their respective owners. phosphor is an original, unaffiliated
work; historical product and feature names in this document are used only
to describe what inspired it, and phosphor implements no compatibility
with those products or their file formats.*

# The phosphor manual

*This file is generated from the in-app help (`phosphor --manual`).
Press F1 inside phosphor for the same manual, opened to the topic
for whatever screen you are on. Do not edit by hand — edit
`src/help.rs` and regenerate.*

## Welcome

```text
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
  · F1 opens this help on the topic for wherever you are.
    Left and Right arrows move between topics; Esc leaves.
  · The dot prompt (press .) accepts real SQL and short commands.
  · q quits from the top level; Ctrl-Q quits from anywhere.

Everything you build — forms, queries, reports, menus — is saved in
small _phosphor tables inside the database file. Copy the file and
the application travels with it.
```

## Browsing & editing

```text
The left panel lists tables (▪) and views (◇). Move with the arrow
keys — or just type a letter to jump to the next table starting
with it — and press Enter to open one in BROWSE, a grid that loads
rows as you scroll, so a million-row table opens instantly.
phosphor's own machinery (shadow tables, _phosphor catalogs, the
dbhealth views) stays hidden; press i to reveal it.

In the grid:

  · Arrow keys move cell by cell; PgUp/PgDn move a screenful;
    g jumps to the first row and G to the last.
  · Enter opens the current row in EDIT — a record form. Choose a
    field, press Enter to type a new value, and press F10 (or
    Ctrl-S) to save. Empty input means NULL. A ¶ marks the primary
    key; a * marks a field that must not be left empty.
  · PgUp/PgDn (or ←/→) in the form flip to the previous/next
    RECORD — hold the key and fly through the file, old-school.
    Unsaved edits SAVE as you page; a failed rule holds the page.
  · a adds a NEW record with the same form. Fields you leave blank
    take the database's own defaults.
  · x deletes the current row — but asks you to press x a second
    time on the same row before anything happens. Any other key
    disarms it.
  · Type  find something  at the dot prompt to jump to the next row
    containing that text in any column; n repeats the search.

If a crafted form exists for the table (see Forms), EDIT uses it:
your field order, your labels, your required rules, and — if you
painted one — your screen layout.

Views and query results open read-only; phosphor tells you so in
the title bar rather than letting a save fail later.
```

## The dot prompt

```text
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
  qbe [table]       Query By Example designer
  report [name]     report designer (a table or a saved report)
  labels [table]    mailing labels, three across
  form [table]      form designer (F2 inside it paints)
  apps / app        application designer / run an application
  run <name>        run a query saved from QBE
  health            the DBHEALTH console
  set theme <name>  green, amber, paper, or blue

Comforts: Up/Down walk your history, Tab completes table names and
commands, Ctrl-A/Ctrl-E jump to the ends of the line, Ctrl-U clears
it, Ctrl-W deletes the previous word.
```

## Query By Example

```text
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

F2 runs the query into the grid. F6 asks for a name and saves the
query into the database; after that,  run <name>  at the prompt
executes it, reports can use it as a source, and application menus
can point at it.
```

## Reports & labels

```text
Press R on a table (or type report at the prompt) to design a
banded report — the kind that produced forty years of business
paperwork: a page header with title and page number, detail lines,
and totals at the bottom.

Three settings, worth exactly three lines on the screen:

  title     Enter to edit. Appears on every page.
  source    a table name, a saved query's SQL, or any SELECT.
  group by  Space cycles through the source's columns. Grouping
            sorts the report, starts a band at each new value, and
            prints subtotals per group.

Columns whose values are all numbers total automatically — per
group and grand. Column widths adapt to the data, and are always
wide enough for their own totals.

F2 previews the report in a pager: arrows and PgUp/PgDn scroll,
w writes the report to a text file, Esc returns. F6 saves the
design; application menus can run it by name.

Labels: press L on a table for mailing labels, three across, every
visible column on its own line — Avery energy, zero configuration.
```

## Forms & the painter

```text
Press F on a table to craft its entry form. The designer lists every
column with four properties:

  SHOW      Space — hidden fields disappear from EDIT entirely.
  REQ       r — required fields refuse to save while empty.
  LABEL     Enter — call the column what humans call it.
  order     [ and ] move the field up and down.

F6 saves; from then on EDIT and NEW use your form for that table.

Press F2 for the FORM PAINTER — CREATE SCREEN, reborn. Your fields
appear on a canvas exactly the size the form will render:

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
until you leave without saving.
```

## Applications

```text
Press A to open the Applications Generator: craft a menu, hand the
database to your team, and it opens as an application.

Each menu item has a label, a kind, and a target:

  browse    opens a table in the grid (with its crafted form)
  query     runs a query saved from QBE, by name
  report    runs a saved report (or a plain table report), by name
  sql       executes a statement — good for one-key housekeeping

In the designer: n adds an item, Enter edits the label, e edits the
target, c cycles the kind, [ and ] reorder, x deletes. Everything
saves as you go. F2 opens the live menu to try it.

The menu itself is pure 1988: arrow keys and Enter, or press the
bright first letter of an item to run it instantly.

To ship it:   phosphor --app yourfile.db
The menu comes up first, and Esc from the top level always returns
to it — the database IS the application. Since menus, forms, and
reports live in _phosphor tables inside the file, copying the file
deploys the app, and libSQL replication deploys it everywhere.
```

## The DBHEALTH console

```text
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
history compresses to about two megabytes.
```

## Connecting

```text
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
```

## Key reference

```text
Everywhere
  F1 help · Esc back out · Ctrl-Q quit · . dot prompt
  Tab cycle focus · F10 dbhealth console

Table list
  ↑↓ move · letters seek · i internals · Enter browse
  r refresh · q quit

BROWSE grid
  ↑↓←→ / hjkl move · PgUp PgDn page · g G first/last row
  Home End first/last column · Enter edit row · a add row
  x (twice) delete row · n find next · F5 refresh
  Q qbe · R report · L labels · F form · A applications

EDIT / NEW record
  ↑↓ field · PgUp/PgDn (or ←→) previous/next record
  Enter edit value · Enter again commit
  F10 / Ctrl-S save · Esc cancel value, then close

Dot prompt
  Enter run · ↑↓ history · Tab complete
  Ctrl-A/E line ends · Ctrl-U clear · Ctrl-W delete word

Form painter
  Tab field · arrows cursor · Space place · t text · b box
  x delete · +/- width · F6 save

Pager (reports, labels)
  ↑↓ PgUp PgDn scroll · g G ends · w write file

Help
  ←→ topics · ↑↓ PgUp PgDn scroll · Esc close
```

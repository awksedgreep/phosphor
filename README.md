# phosphor

**The green-screen database desktop of 1988, reborn. Dot prompt, banded
reports, painted forms, user-built menus — on top of a database that is
wicked fast, compressed, networked, and monitors itself. 1988 the way it
should have turned out.**

```
┌──────────────────────────────────────────────────────────────┐
│  PHOSPHOR                                     [F1 Help]      │
│                                                              │
│   Data      Queries     Forms      Reports    Apps    Admin  │
│   ────      ───────     ─────      ───────    ────    ─────  │
│   customers CRM-open    cust-entry monthly    CRM     health │
│   orders    late-pay    order-fast aging      intake  logs   │
│   metrics   <create>    <create>   labels     <create>       │
│                                                              │
│  . _                                                         │
│                                                              │
│  ─ dot prompt ────────────────────── db: crm.db ── 0.19ms ─  │
└──────────────────────────────────────────────────────────────┘
```

## The dream

Twenty years of dreaming, stated plainly: terminals never stopped being the
fastest UI humans have ever had. The great late-80s database desktops —
dBASE IV, Paradox, FoxPro — let a non-programmer build a real business
application — data, forms, reports, menus — in an afternoon, and every
keystroke responded *instantly*. We traded that for web apps with 400 ms
round trips and forms built by committees.

phosphor brings it back, and forward:

- **The dot prompt.** A live command line for SQL and app commands, with
  history. The fastest interface ever shipped with a database.
- **BROWSE and EDIT.** Grid-edit any table; flip to a record form with one
  key. Master-detail linked browses (`SET RELATION`, reborn as foreign
  keys driving the UI).
- **Painted forms.** A full-screen form designer — place fields, labels,
  pickers, validations — saved *into the database itself*.
- **Banded reports.** Page header, group bands, detail, totals, footer —
  the report writer that generated forty years of business paperwork,
  plus mailing labels.
- **Query By Example.** A QBE grid that writes the SQL for you and shows
  it — the original "low-code" done honestly.
- **The Applications Generator.** Users craft their own menus wired to
  their own forms, queries, and reports — then hand the result to their
  team as *an application*. The fastest CRM in existence, built by the
  person who actually uses it.
- **DB intelligence built in.** Powered by
  [timeless-libsql](https://github.com/awksedgreep/timeless-libsql):
  compressed metrics/logs/traces in the same file, and a `dbhealth`
  report that tells you — in plain language — whether the database is
  healthy and what to do about it. PMM energy, F10 away.
- **Two ways to connect, one UI:** open a SQLite/libSQL file directly
  (embedded, zero infrastructure, microsecond queries) or speak to a
  self-hosted `sqld` over HTTP (multi-user — the Novell NetWare of this
  story, minus the lock files).
- **The apps live in the database.** Forms, menus, reports, queries are
  rows, not files on someone's C: drive. Copy the `.db`, you copied the
  application. Replicate it with libSQL, you *deployed* it.

Green P1 phosphor by default. Amber P3 for the sophisticates. `Esc` always
means what you think it means.

## Status

**All five phases work today** — the browser, the network, the health
console, the builders, and the applications runtime:

```sh
cargo run -- path/to/any.db            # embedded: any SQLite/libSQL file
cargo run -- http://localhost:8880     # remote: self-hosted sqld over HTTP
# PHOSPHOR_TOKEN=...                     for authenticated servers (Turso-style)
# PHOSPHOR_EXT=.../libtimeless_ext.so    embedded telemetry + dbhealth
```

- **Browse** — schema sidebar (tables ▪, views ◇), virtualized **BROWSE**
  grid that pages through millions of rows, **EDIT** record form on Enter
  (PICTURE-style ¶ pk / * not-null markers, typed parsing), a live **dot
  prompt** (`.`) running real SQL with history, four themes
  (`set theme green|amber|paper|blue`), F1 help, query latency in the
  status bar.
- **Network** — the same UI over Hrana HTTP to self-hosted
  [sqld](https://github.com/tursodatabase/libsql): one `DbLink` trait,
  two backends, chosen by the argument. Multi-user, no lock files —
  the part 1988 got wrong, fixed.
- **DBHEALTH console** — `F10` (or `health` at the prompt) on a database
  carrying [timeless-libsql](https://github.com/awksedgreep/timeless-libsql)
  telemetry: the plain-language health report (worst first), sparkline
  trends fed from the compressed series, and `s` to take a live sample
  right there — works identically over a file or over sqld. The status
  bar carries the health dot at all times.
- **The builders** — `Q`uery By Example (fill the grid, watch the SQL it
  writes, F2 runs, F6 saves), `R`eports (banded: page headers, group
  bands with subtotals, automatic totals on numeric columns, grand
  totals; preview in a pager, `w` writes the file), `L`abels
  (three-across, zero config), and `F`orms (reorder, relabel, hide, and
  require fields — EDIT uses your form from then on).
- **The Applications Generator** — press `A`, craft a menu of actions
  (browse a table, run a saved query or report, execute SQL), and the
  result is an *application* stored in `_phosphor_*` tables inside the
  database itself. Then:

  ```sh
  phosphor --app crm.db      # your team's CRM, hotkeys and all
  ```

  Copy the file, you copied the app. Replicate it with libSQL, you
  deployed it.

See [DESIGN.md](DESIGN.md) for the feature revival map and architecture.

## License

[MIT](LICENSE)

*phosphor is an original work inspired by the terminal database tools of
the late 1980s. It is not affiliated with, endorsed by, or compatible with
dBASE® (a trademark of dBase, LLC), Paradox, FoxPro, or their successors;
historical product names appear only for comparison. phosphor works
exclusively with SQLite and libSQL databases.*

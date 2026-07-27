# phosphor

**dBASE IV reborn. Green screen, dot prompt, banded reports, painted forms,
user-built menus — on top of a database that is wicked fast, compressed,
networked, and monitors itself. 1988 the way it should have turned out.**

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
fastest UI humans have ever had. dBASE IV's Control Center let a
non-programmer build a real business application — data, forms, reports,
menus — in an afternoon, and every keystroke responded *instantly*. We
traded that for web apps with 400 ms round trips and forms built by
committees.

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

**Pre-alpha: a splash screen and a manifesto.** The design is real, though —
see [DESIGN.md](DESIGN.md) for the dBASE IV feature revival map, the
architecture, and the phasing.

```sh
cargo run   # q or Esc to exit — that part works already
```

## License

[MIT](LICENSE)

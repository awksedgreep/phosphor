# Build a CRM in ten minutes

The whole promise of phosphor in one exercise: start from an empty file,
end with an application your team can run. No code — just keystrokes,
and every keystroke is listed. The GIFs linked along the way come from
the [UI test suite](UI-TOUR.md), so they show exactly what you'll see.

You'll need the phosphor binary (`cargo build --release`). Optional but
recommended: the dbhealth extension, so your CRM monitors itself —

```sh
export PHOSPHOR_EXT=/path/to/libdbhealth_ext.so
```

## 1 · Open a database that doesn't exist yet

```sh
phosphor crm.db
```

phosphor creates the file and shows an empty table list. Two ways to
give your CRM bones — press **`C`** for the TABLE DESIGNER (type
field names directly, F-keys set types and constraints, and the
CREATE TABLE writes itself underneath — `F2` builds it), or press `.` and speak SQL directly (paste both, one at a
time):

```sql
CREATE TABLE customers(id INTEGER PRIMARY KEY, name TEXT NOT NULL UNIQUE, city TEXT, balance REAL DEFAULT 0)
```
```sql
CREATE TABLE orders(id INTEGER PRIMARY KEY, customer TEXT NOT NULL REFERENCES customers(name), product TEXT, qty INTEGER, amount REAL, region TEXT)
```

That `REFERENCES` is the good stuff — in the TABLE DESIGNER it's
`F10` on a field. Declare it and phosphor gives you dBASE's SET
RELATION for free: open a customer and their orders appear in a
pane under the form (keep reading).

The tables appear in the sidebar as you create them. *(While you're
here: if you loaded the extension, `CREATE VIRTUAL TABLE dbhealth USING
dbhealth` gives the database a pulse — the ● in the status bar.)*

## 2 · Put some customers in it

`Esc` to the sidebar, then type `c` — first-letter seek jumps to
`customers`. `Enter` opens BROWSE. It's empty; press **`a`** to add a
record:

| keys | what happens |
|---|---|
| `a` | a NEW record form opens |
| type `Ada`, `Enter` | Name filled — the form is live, just type (it's marked `*` — required) |
| `Tab`, type `London`, `Enter` | City filled |
| `Enter` again on the last field | commits — and **saves**; Enter is the save key (F10 saves-and-closes) |
| `PgDn` / `PgUp` (in the form) | flip through records — hold it down and fly; edits save as you page |

Add two or three more. Try saving one with a blank Name — phosphor
refuses, politely. And once orders exist, notice the **orders pane**
under each customer: that's the foreign key at work — it refreshes
live as you page, and `F4` opens it as a filtered BROWSE.
*(Watch it: [relations.gif](demo/ui/relations.gif))* That rule came free from `NOT NULL`; you'll add your
own rules next. *(Watch it: [crud.gif](demo/ui/crud.gif))*

## 3 · Craft the entry form

Your team shouldn't see raw column names. On `customers`, press **`F`**:

| keys | what happens |
|---|---|
| `Space` on `id` | hidden — nobody types ids |
| `↓` to `name`, `Enter`, retype `Customer`, `Enter` | your label, not the column's |
| `r` on the same row | required, enforced at save |
| `F6` | saved — EDIT uses this form from now on |

Now press **`F2` — the FORM PAINTER**. Your fields sit on a canvas the
exact size the form will render:

| keys | what happens |
|---|---|
| `Tab` | select a field (the cursor jumps to it) |
| arrows, then `Space` | walk the canvas, drop the field there |
| `t`, type `CUSTOMER CARD`, `Enter` | a title, placed at the cursor |
| `b`, move down-right, `b` | a box, corner to corner |
| `F6` | saved — open any customer and see *your screen* |

*(Watch it: [forms.gif](demo/ui/forms.gif))*

## 4 · Save the queries your team actually runs

On `customers`, press **`Q`** — Query By Example. One line per column,
and the SQL you're generating stays visible at the bottom (that's the
point):

| keys | what happens |
|---|---|
| `↓↓↓` to `balance`, `Enter`, type `> 100`, `Enter` | a filter |
| `s` `s` | sort descending ▼ |
| `F2` | run it — your best customers, in the grid |
| `Q`, redo the filter, `F6`, type `big-spenders`, `Enter` | saved into the database |

From now on, `run big-spenders` at the dot prompt replays it — and menus
can point at it. *(Watch it: [qbe.gif](demo/ui/qbe.gif))*

## 5 · Design the report the boss wants

On `orders` (seek with `o`), press **`R`**:

| keys | what happens |
|---|---|
| `Enter`, edit the title, `Enter` | appears on every page |
| `↓↓`, then `Space` until it says `region` | group bands + subtotals per region |
| `F2` | preview: page header, ▌ bands, totals (ids are never summed) |
| `w` | writes `report_orders.txt` for printing/mailing |
| `Esc`, `F6` | saved by name |

Labels too, if you mail things: `L` on customers — three-across, done.
*(Watch it: [reports.gif](demo/ui/reports.gif))*

## 6 · The Applications Generator

This is the 1988 magic. Press **`A`**:

| keys | what happens |
|---|---|
| `n` | a new menu item (already selected) |
| `Enter`, retype `Customers`, `Enter` | its label — first letter becomes the hotkey |
| `e`, type `customers`, `Enter` | its target (a table to browse) |
| `n` again → label `Big spenders`, `c` until kind reads `query`, `e` → `big-spenders` | a saved-query item |
| `n` again → label `Orders by region`, `c` to `report`, `e` → `orders` | a report item |
| `F2` | the live menu — try the hotkeys |

Everything you just built — form, query, report, menu — is rows in the
database. *(Watch it: [apps.gif](demo/ui/apps.gif))*

## 7 · Ship it

```sh
phosphor --app crm.db
```

The ▓▓ CRM ▓▓ menu comes up first. Hotkeys run everything; Esc always
comes home. Hand the *file* to your team — copy it, mail it, or serve it
multi-user through [sqld](https://github.com/tursodatabase/libsql) with
`phosphor http://host:8880` — the application travels inside it either
way. *(Watch it: [appmode.gif](demo/ui/appmode.gif))*

Ten minutes. No code. dBASE users did this in 1988 and we all somehow
agreed to forget it was possible. Press `F1` anywhere for the rest of
the manual — or read it on the web: [MANUAL.md](MANUAL.md).

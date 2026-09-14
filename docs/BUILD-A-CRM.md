# Build a CRM from an empty database

This walkthrough creates customers and orders, enters records, saves an entry form,
query and report, and connects them to a CRM menu. The `tutorial` terminal regression
in `tools/demo/uitest.py` follows these steps at 80×24 against a fresh database,
then starts a second process to reopen the application. The GIFs elsewhere in the
repository illustrate earlier recordings; use the keystrokes below for this version.

## 1 · Start with a file that will keep your work

From the repository folder:

```sh
cargo build --release
./target/release/phosphor crm.db
```

Use a **new filename** for this exercise. If you installed `phosphor` on your PATH,
`phosphor crm.db` works too. No extension or server is required. The file is created
immediately; saved records and designs stay in it after you quit.

Starting without a filename opens **TEMPORARY SCRATCH**, which disappears when you
quit. To keep a scratch session, save your current record or design, return to the
browser, and press **F9 Save Database**. Enter a new filename. Existing files are
refused. After success, phosphor continues working in the new file, so subsequent
saved changes persist there too.

## 2 · Create customers

The empty screen offers **C Create your first table**. Capital letters below mean
Shift plus that letter. A name in backticks after “type” is text to enter, without
backticks. Function keys may need Fn on your keyboard.

| Keys | Result |
|---|---|
| `C`, `Home`, type `customers`, `Enter`, `Tab` | Name the table; select its supplied `id INTEGER PRIMARY KEY`. |
| `F8`, type `name`, `Enter`, `F5`, `F6` | Add a TEXT name; make it required and unique. |
| `F8`, type `city`, `Enter` | Add a TEXT city. |
| `F8`, type `balance`, `Enter`, `F3` | Add balance and change TEXT to REAL. |
| `F7`, type `0`, `Enter` | Give balance a default of zero. |
| `F2` | Build the table and open its empty BROWSE. |

The SQL preview shows the structure before you apply it. Later, **E** opens the
Table Editor to add, rename, or drop columns; changes that need a lossy rebuild
are refused rather than discarding constraints or triggers.

## 3 · Enter your first records

Press **a once** to open NEW customers. The cursor starts on `name`; `id` shows
`(automatic)`. You can deliberately enter an ID with Ctrl-Home, but none is needed
for this exercise.

| Keys | Result |
|---|---|
| `F10` before entering a name | The required-name error leaves NEW open. |
| type `Ada`, `Enter` | Insert Ada with an automatic ID and default balance; move to city. |
| type `London`, `Enter` | Save city and move to balance. |
| type `120.5`, `Enter`, `F10` | Save the balance and close the form. |
| `a`, type `Grace`, `Enter` | Add the next customer. |
| type `Arlington`, `Enter`, type `80`, `Enter`, `F10` | Finish Grace and close. |

**Enter saves and advances.** There is no extra Tab between these entries. F10
saves and closes; on a clean form it simply closes. A duplicate name or another
failed constraint keeps the draft open for correction. Fields left untouched use
the database defaults. In an existing form, PgUp/PgDn moves between records and
saves edits as you move. F1 Help and F12 error details preserve your draft.

## 4 · Create orders and choose a customer by name

From BROWSE customers, press **Esc** to return to the table list.

| Keys | Result |
|---|---|
| `C`, `Home`, type `orders`, `Enter`, `Tab` | Name the new table and keep its automatic ID. |
| `F8`, type `customer`, `Enter`, `F5` | Add a required TEXT customer field. |
| `F10`, type `customers(name)`, `Enter` | Reference the unique customer name. |
| `F8`, type `product`, `Enter` | Add a TEXT product. |
| `F8`, type `qty`, `Enter`, press `F3` four times | Change the new field from TEXT to INTEGER. |
| `F8`, type `amount`, `Enter`, `F3` | Add a REAL amount. |
| `F8`, type `region`, `Enter`, `F2` | Add a TEXT region and build the table. |

Press **a**, then **F7** on the customer field. The picker shows Ada and Grace.
Press **Enter** to choose Ada, then **Tab** to move to product. Type `modem`, Enter;
`2`, Enter; `40`, Enter; `east`, Enter; then **F10** to close. The saved order is
linked to Ada. For a large customer list, `/` searches names and details, and
PgUp/PgDn or Home/End reaches records beyond the first page.

## 5 · Craft the customer entry form

From BROWSE orders, press **Esc**, then **c** to select customers and **F** to design
its form. The form designer starts on id.

| Keys | Result |
|---|---|
| `Space` | Hide id from this entry form. |
| `Down`, `Enter`, type `Customer`, `Enter`, `r` | Label name as Customer and make it required in the form. |
| `F6` | Save the form. |
| `F2`, `Up`, `t`, type `CUSTOMER CARD`, `Enter` | Open the painter and add a caption above the fields. |
| `F6`, `Esc`, `Esc` | Save the painted form and return through the designer to the table list. |

Tab selects a field in the painter; arrows and Space place it. Oversized layouts
use a scrolling list at runtime when the terminal cannot fit the saved canvas.

## 6 · Save a useful query

With customers still selected, press **Q** for Query By Example.

| Keys | Result |
|---|---|
| `End`, `Enter`, type `> 100`, `Enter` | Filter the last column, balance. |
| `s`, `s` | Sort descending. |
| `F2` | Preview Ada, the one matching customer. |
| `Esc`, `F6`, type `big-spenders`, `Enter`, `Esc` | Return to the design, save it, and return to the table list. |

At the dot prompt, `run big-spenders` runs the query and `qbe big-spenders` reopens
its design, including after restart. F6 updates a design; F7 makes a separate copy;
F8 renames it. Existing names are protected.

## 7 · Save a grouped report

From the table list, press **o**, then **R**.

| Keys | Result |
|---|---|
| `Enter`, type `Orders by region`, `Enter` | Set the report title. |
| `Down`, `Down`, `Enter`, type `region`, `Enter` | Set the grouping field explicitly. |
| `F6`, type `orders-by-region`, `Enter` | Save the named report before previewing. |
| `F2` | Preview the east group, subtotal, and grand total for one order. |
| `w` | Write `report_orders-by-region.txt` in the launch folder. |
| `Esc`, `Esc` | Return through the report designer to the table list. |

The file contains the complete report. Printing with `p` requires a configured
`lp` command or `PHOSPHOR_PRINT`; printing is optional in this walkthrough.
For mailing labels, select customers with **c**, press **L**, and **Esc** to return.

## 8 · Build the CRM menu

From the table list, press **A**. On a fresh database this opens an empty application
named `app`. Menu items save as you edit them.

| Keys | Result |
|---|---|
| `n`, `Enter`, type `Customers`, `Enter` | Create and label the first item. |
| `e`, type `customers`, `Enter` | Point its browse action at the customers table. |
| `r`, type `CRM`, `Enter` | Rename the application itself to CRM. |
| `n`, `Enter`, type `Big spenders`, `Enter`, `c` | Add a query item. |
| `e`, type `big-spenders`, `Enter` | Point at the saved query. |
| `n`, `Enter`, type `Orders by region`, `Enter`, `c`, `c` | Add a report item. |
| `e`, type `orders-by-region`, `Enter` | Point at the saved report, including its title and grouping. |
| `F2` | Preview the CRM menu. |

Try **c** for Customers, **Esc** back to the menu; **b** for Big spenders, **Esc**;
then **o** for the report and **Esc**. Each action should show the data you entered.
Esc again returns to the application designer with its last selected item intact.

## 9 · Quit and reopen the application

Press **Ctrl-Q**, then run:

```sh
./target/release/phosphor --app CRM crm.db
```

The CRM menu opens with all three items. Press **c**, then **Enter** to see a saved
customer in the CUSTOMER CARD form. The data, form, query, report, and menu are all
inside `crm.db`. Close phosphor before copying the database to share it.

For CSV import/export, Lua rules, and remote connections, use F1 or the
[full manual](MANUAL.md). The optional dbhealth extension has separate setup;
it is not needed to complete this walkthrough.

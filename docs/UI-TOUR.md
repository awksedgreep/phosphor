# The UI tour — every screen, on film

These GIFs are the output of `tools/demo/uitest.py` — the full-UI test
sweep. Each one is a scripted pty session with **on-screen assertions**
(a small terminal emulator reconstructs what is actually visible and
checks expected text at scripted moments). They are only regenerated
from a fully green run, so what you see here is what the tests proved:

```sh
python3 tools/demo/uitest.py --render   # assert everything, then render
```

| reel | covers |
|---|---|
| ![nav](demo/ui/nav.gif) | first-letter seek, the internals toggle, browse motion (g/G, Home/End), read-only views |
| ![columns](demo/ui/columns.gif) | column widths (`+`/`-`) and frozen columns (`f`) |
| ![crud](demo/ui/crud.gif) | the painted card, editing a field, required-field refusal, insert, find, double-x delete |
| ![prompt](demo/ui/prompt.gif) | SQL in the grid, error handling, Tab completion, all four themes |
| ![qbe](demo/ui/qbe.gif) | Query By Example: filters, live (wrapping) SQL, run, save, replay by name |
| ![qbeextras](demo/ui/qbeextras.gif) | QBE teaches the rest: `J` cycles FK joins, `g` adds GROUP BY |
| ![reports](demo/ui/reports.gif) | banded report with group subtotals, save-as (renameable), writing to file, mailing labels |
| ![forms](demo/ui/forms.gif) | the form designer, `n` adds a computed field, the painter (place, text, box), the painted EDIT |
| ![pickers](demo/ui/pickers.gif) | PICTURE masks and the `F7` foreign-key value picker |
| ![apps](demo/ui/apps.gif) | the Applications Generator: add/label/target, live menu, hotkeys, delete, rename the app (`r`) |
| ![create](demo/ui/create.gif) | the TABLE DESIGNER: fields as rows, the CREATE TABLE writing itself, F2 → empty BROWSE → first record |
| ![tableeditor](demo/ui/tableeditor.gif) | the TABLE EDITOR (`E`): add a column with ALTER, and drop a table with the two-press confirm |
| ![paging](demo/ui/paging.gif) | record paging: hold PgDn and fly through 500 records in the form; edits save mid-flight |
| ![relations](demo/ui/relations.gif) | foreign keys → SET RELATION: declare an FK in the designer (F10), open a parent record, and the child pane follows as you page; F4 opens it as a filtered BROWSE; `v` splits and `H` stacks |
| ![scripting](demo/ui/scripting.gif) | Lua scripting: an `OnChange` lifecycle script rewrites a field on save; a `script` menu item authored in the full-screen editor runs by hotkey |
| ![data](demo/ui/data.gif) | CSV import/export at the prompt, then the index/vacuum advisor |
| ![health](demo/ui/health.gif) | the LIVE DBHEALTH console + contextual F1 help |
| ![appmode](demo/ui/appmode.gif) | `--app`: the menu, a report, single-Esc home |
| ![kiosk](demo/ui/kiosk.gif) | `--app --readonly`: browsable, and every write refused |

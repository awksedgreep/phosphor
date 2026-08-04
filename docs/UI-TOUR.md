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
| ![crud](demo/ui/crud.gif) | the painted card, editing a field, required-field refusal, insert, find, double-x delete |
| ![prompt](demo/ui/prompt.gif) | SQL in the grid, error handling, Tab completion, all four themes |
| ![qbe](demo/ui/qbe.gif) | Query By Example: filters, live (wrapping) SQL, run, save, replay by name |
| ![reports](demo/ui/reports.gif) | banded report with group subtotals, writing to file, mailing labels |
| ![forms](demo/ui/forms.gif) | the form designer, the painter (place, text, box), the painted EDIT |
| ![apps](demo/ui/apps.gif) | the Applications Generator: add/label/target, live menu, hotkeys, delete |
| ![paging](demo/ui/paging.gif) | record paging: hold PgDn and fly through 500 records in the form; edits save mid-flight |
| ![health](demo/ui/health.gif) | the LIVE DBHEALTH console + contextual F1 help |
| ![appmode](demo/ui/appmode.gif) | `--app`: the menu, a report, single-Esc home |

**Visible navigation and complete parent lookup — #41 and #42**

Table lists, query/table/form/report designers, application generators, application menus, and record forms now keep the selection in view. Each screen retains its own scroll offset and adapts it when the terminal resizes. PgUp/PgDn and Home/End navigate lists and designers; sidebar mouse clicks account for the scrolled position. Mouse events over an overlay no longer change the browser underneath it.

Record forms distinguish field navigation from record navigation. Up/Down visits every displayed field, including computed and generated values; Tab/Shift-Tab continues to skip read-only fields. Alt-PgUp/PgDn pages through fields, and Ctrl-Home/End reaches the first or last field. Existing PgUp/PgDn and Left/Right still save and page through records. Required-field and picture-mask validation brings the offending field into view.

The form painter scrolls with its cursor. Auto-placement extends the canvas for additional fields instead of stacking them on its last row. Runtime forms use a scrolling list when the saved canvas or field positions cannot fit, retaining the crafted labels, order, masks, and validation. Long single-line input has a full-width editing line above the prompt. Script and note editors scroll in both directions to follow the caret; the dot prompt follows its caret horizontally. Text clipping accounts for Unicode graphemes and terminal cell widths. Help wraps to the available width, hides its topic list on narrow screens, and can reach the bottom of the wrapped text.

F7 foreign-key lookup now loads bounded pages of 100 records with an absolute position and complete match count. PgUp/PgDn changes pages; Home/End reaches the first or last match. Press `/`, enter a name, key, or other parent detail, and press Enter to search. Searches match literal substrings across the parent's columns, so quotes, percent signs, and underscores remain ordinary search text. SQLite's `lower` supplies ASCII case folding. The key stays in the first column; name/title/label and text columns receive priority, and Left/Right reveals additional descriptive columns.

Lookup runs through the database worker. Loading and failure states prevent selecting stale results; F5 retries a failure. Cancelled and superseded responses cannot reopen the picker or overwrite a newer search. Help preserves the lookup and unfinished search. Enter writes the original typed database value into the record draft. Empty text, multiline text, and binary keys retain their exact values through validation, save, and a cancelled field edit. Other draft fields remain intact.

The optional create-parent action mentioned in #42 is not included; the picker selects existing parents. Search and count still scan the matching data, and OFFSET paging can become slower on very large tables. The bounded cache limits returned rows, rather than promising constant-time lookup. Counts and pages are separate reads, so a concurrent parent-table change may require F5 refresh.

**Validation**

- All 206 Rust suite entries passed with all features and official sqld 0.24.32 enabled. Two optional dbhealth integrations return early because the extension is unavailable.
- Shared application/worker regressions run on embedded SQLite, the rotating Hrana fixture, and real sqld. A 1,005-parent fixture verifies paging past 200, jumping to the end, descriptive and literal-text search, empty results, retained drafts, exact key values, cancelled input, stale responses, lookup failure, Help during loading, and successful retry.
- Renderer and keyboard regressions cover 40 tables, 35 fields, 40 menu items, long Unicode input, a 60-line script, and wrapped Help. They resize between 80×24, 40×12, and 100×30; additional cases exercise a 25×8 sidebar and a tall painted canvas. Required-field failure keeps the form open and reveals the invalid field.
- All 31 available embedded terminal workflows passed across the full sweep and focused final reruns. The new `scrolling` and `lookup` reels use real terminal resize events and verify saved database values. Five existing affected workflows were also rerun against the final implementation.
- Nine real-sqld terminal workflows passed across the sweep and final rerun: `crud`, `forms`, `pickers`, `generated`, `detailcrud`, `builders`, `status80`, `scrolling`, and `lookup`. The two dbhealth-dependent reels remain unavailable.
- Formatting, Clippy with all targets/features and warnings denied, generated-manual synchronization, the release build, and both release performance budgets passed.

The terminal recorder now supports resize events and drains output while feeding large input, avoiding a recorder deadlock when fixture SQL fills the pseudoterminal. Recordings remain in disposable directories. The full terminal sweep is still a separate local check; CI includes the Rust regressions and required real-sqld workflow.

**Reproduce**

```sh
cargo test --all-features --quiet
python3 tools/test_sqld.py
cargo clippy --all-targets --all-features -- -D warnings
cargo fmt --check
cargo run --quiet --bin phosphor -- --manual | diff -u docs/MANUAL.md -
cargo build --release
cargo test --release perf_budget
python3 tools/demo/remote_uitest.py --bin /path/to/sqld --reels crud forms pickers generated detailcrud builders status80 scrolling lookup
```

For the new embedded terminal workflows without writing recordings into the checkout:

```python
import json, sys, tempfile
from pathlib import Path
sys.path.insert(0, 'tools/demo')
import uitest as ui

root = Path(tempfile.mkdtemp(prefix='pf-navigation-', dir='/tmp'))
ui.DB, ui.OUT, ui.WORK = [str(root / n) for n in ('ui.db', 'casts', 'work')]
ui.ENV = {'PHOSPHOR_EXT': '', 'PHOSPHOR_TOKEN': ''}
results = {}
for reel in ui.reels():
    if reel.name not in {'scrolling', 'lookup'}:
        continue
    ui.seed()
    results[reel.name] = reel.run()
(root / 'results.json').write_text(json.dumps(results, indent=2))
print(root, results)
assert not any(results.values())
```

SQL rewriting, first-run guidance, saved-asset discovery, event-loop latency, and the other follow-ups remain tracked in #43–#47.

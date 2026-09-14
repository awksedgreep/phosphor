**SQL previews and the first usable application — #43 and #44**

Interactive queries execute without an appended LIMIT. PRAGMA, VALUES, EXPLAIN, CTEs, leading and trailing comments, string literals containing `limit`, and existing LIMIT/OFFSET clauses keep their SQLite meaning. The dot prompt recognizes query keywords after leading comments. Each result-producing request must contain exactly one statement; a multi-statement query is rejected before any of its statements run. Failed queries leave the preceding result grid available.

Embedded previews read at most 10,001 rows, retain 10,000, and use the extra row to show the existing truncation notice accurately. Remote reads use Hrana's cursor endpoint with the same retained-row limit. An autocommit SELECT preview can stop and close its own cursor after the extra row. A caller-owned transaction drains the response to preserve its continuation and rollback capability. Writes with RETURNING complete before their displayed rows are truncated. These writes still use the ordinary remote response buffer. Sorting, aggregation, and transaction-owned reads can require work beyond the displayed rows; the preview cap is not a constant-time execution guarantee. Complete exports, reports, and labels retain their existing streaming path. Remote read-only mode retains its SELECT-subquery protection and uses `pragma_*` table functions for metadata.

The empty browser now offers **C Create your first table**, followed by F2 to build it and a to enter the first record. NEW starts on a writable field without an automatic identity or default when one is available. Automatic INTEGER PRIMARY KEY values show `(automatic)` until explicitly entered. Computed values and fixed parent links are skipped. Manual text, composite, and descending primary keys retain their entry behavior, and Ctrl-Home allows a deliberate manual ID. Existing Enter behavior saves and advances, so the guide no longer inserts an extra Tab between field values.

No-argument sessions prominently explain that scratch data disappears on quit. **F9 Save Database** asks for a new filename and continues working in that file after success. The SQLite backup preserves records, implicit row IDs, schema, triggers, and saved application catalogs. Publication refuses existing destinations, including a file created during the save. A failed save retains the scratch connection and filename for retry. If publication succeeds but reopening fails, the message distinguishes the saved file from the still-open scratch connection. Open transactions must finish first; attached databases must be detached, and temporary objects moved or dropped, because the operation copies only the main database. Empty temporary schemas do not prevent saving. Help and status details preserve the filename, and the input remains visible when resized to 40×12.

The [CRM walkthrough](../BUILD-A-CRM.md) now starts with an empty named database and follows a complete, tested path: create customers, enter Ada and Grace with automatic IDs, create an order and choose Ada by name, save and paint the customer form, preview and save a query, save and write a grouped report, create a menu referencing those saved assets, quit, and reopen the CRM in a second process. The final database and written report were checked for exact customer/order values, foreign-key integrity, saved labels/layout, filter/sort, grouping, and menu targets. Earlier GIFs remain illustrative recordings rather than the keystroke reference for this version.

**Validation**

- All 213 Rust suite entries passed with all features and official sqld 0.24.32 enabled. Two optional dbhealth integrations return early because the extension is unavailable.
- Shared application/worker regressions run against embedded SQLite, the rotating Hrana fixture, and real sqld. They cover native query syntax, cap boundaries, duplicate column names, existing LIMIT/OFFSET, failed SQL, preserved transaction ownership, complete writes with RETURNING, and first-entry success/failure with defaults and automatic/manual keys. A truncated remote response preserves the previous grid and a subsequent query recovers.
- Scratch-save regressions verify preserved negative/zero/nonconsecutive row IDs, schema, triggers, saved forms/menus/preferences, writes after switching to the file, reopen, existing-file protection, publication races, invalid paths, transactions, attached/temporary objects, cancellation, Help, and narrow filename rendering.
- All 34 available embedded terminal workflows passed across the full sweep and focused final reruns. The new `querysyntax`, `scratch`, and `tutorial` reels cover SQL success/recovery, filename failure/retry and process restart, and the literal 80×24 CRM walkthrough. Four older reels were updated for automatic initial-field selection while retaining explicit-ID coverage.
- All 21 real-sqld terminal workflows passed, including the new SQL syntax/recovery and complete CRM walkthrough/restart reels. The sweep also covers imports, scripting, forms, menus, read-only protection, schema editing, record identities, related records, complete output, builder continuity, visible status, resizing, and parent lookup.
- Formatting, Clippy with all targets/features and warnings denied, manual synchronization, release build, and both release performance budgets passed.

The two dbhealth-dependent terminal reels remain unavailable. Printing and telemetry setup remain separate follow-ups; this batch validates writing the report file, not sending it to a printer. Automated guide coverage also does not replace observing an unfamiliar person use the software. Saved-asset discovery and event-loop latency remain tracked in #45 and #46; the other review follow-ups remain in #47.

**Reproduce**

```sh
cargo test --all-features --quiet
python3 tools/test_sqld.py
cargo clippy --all-targets --all-features -- -D warnings
cargo fmt --check
cargo run --quiet --bin phosphor -- --manual | diff -u docs/MANUAL.md -
cargo build --release
cargo test --release perf_budget
python3 tools/demo/remote_uitest.py --bin /path/to/sqld
```

Run the three new embedded terminal workflows in disposable directories:

```python
import json, sys, tempfile
from pathlib import Path
sys.path.insert(0, 'tools/demo')
import uitest as ui

root = Path(tempfile.mkdtemp(prefix='pf-onboarding-', dir='/tmp'))
ui.DB, ui.OUT, ui.WORK = [str(root / n) for n in ('ui.db', 'casts', 'work')]
ui.ENV = {'PHOSPHOR_EXT': '', 'PHOSPHOR_TOKEN': ''}
results = {}
for reel in ui.reels():
    if reel.name not in {'querysyntax', 'scratch', 'tutorial'}:
        continue
    ui.seed()
    results[reel.name] = reel.run()
(root / 'results.json').write_text(json.dumps(results, indent=2))
print(root, results)
assert not any(results.values())
```

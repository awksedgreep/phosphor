**Builder continuity and visible outcomes — #39 and #40**

QBE results, report previews, and application-menu previews now retain their originating designer. Esc returns to that design with its settings and cursor intact; Q also returns from QBE results. A failed query or report keeps the designer open. QBE responses are discarded if the user cancels or revises the design before the response arrives. Help remains available throughout the preview round trip. The browser stays live behind reports and application menus, and QBE restores its previous browser after displaying results.

Saved QBE designs can now be reopened with `qbe <saved-name>`, including after restarting. The loader restores filters, projection, sort, joins, and grouping, while rediscovering available relationships. Invalid JSON, unsupported versions, and missing saved columns or relationships produce an error without replacing the current screen or rewriting the saved SQL. A saved query name takes precedence over a table of the same name, matching report opening behavior.

Queries and reports now distinguish three actions:

| Action | Behavior |
|---|---|
| F6 Save | Ask for a name on the first save; subsequently update that design. |
| F7 Save As | Create a separate copy and keep the original. |
| F8 Rename | Move the saved design and update matching application-menu references. |

Save As and Rename refuse an existing name. To replace a saved design deliberately, reopen it and use F6. Catalog changes and menu-reference updates share a savepoint, so a failure cannot leave a partially renamed application. A failed save retains both the design and any name being entered for correction or retry. The old report F6 behavior that silently removed the original after changing the name has been removed.

The other builders retain their existing persistence models: forms belong to their table and F6 saves them; Esc from the painter retains the form in its list designer. Application items save as they are edited, and previewing now returns to the selected item. Table F2 applies the structure to the database, where E reopens it. Labels have no editable design settings. The manual and the CRM guide now describe the actual query/report preview, revision, save, and reopen keystrokes.

Status messages now precede connection information. Errors and confirmations receive the available width; row position appears only when it fits without shortening the message. Long messages show an ellipsis and an always-visible F12 details hint. When there is no message, local connections use the file basename and remote connections use the authority instead of the full path.

F12 opens the full, wrapped, scrollable message and connection details from any screen, including while typing or reading Help. Esc or F12 returns without changing the draft. Reading a destructive confirmation does not disarm it; the second delete still applies to the same record. The status line is drawn after overlays so a tall designer cannot hide it.

**Validation**

- All 201 Rust suite entries passed with all features and official sqld 0.24.32 enabled. Two optional dbhealth integrations return early because the extension is unavailable.
- Shared App/worker tests use a reopened file, the rotating Hrana fixture, and real sqld to verify query/report previews, revision, separate copies, collision refusal, successful saves, restart/reopen, retained form layouts, and persisted application references. A trigger-induced menu-update failure rolls back the rename; a trigger-induced report-save failure retains the revision for retry.
- Additional tests cover cancelled and failed QBE results, failed report sources, malformed saved QBE JSON, Help during a pending preview, and detail requests completing behind a report preview.
- At 80×24, renderer tests with a long local path and a long remote URL verify required-field validation, database constraint failures, successful saves, full connection details, long-message scrolling, retained typing, and deletion after inspecting its confirmation.
- Formatting, Clippy with all targets/features and warnings denied, generated-manual synchronization, the release build, and both release performance budgets passed.
- All 29 available embedded terminal workflows passed using a database path longer than 200 characters. The initial sweep exposed an obsolete application-menu navigation assertion; it was updated to use the preserved designer cursor. Seven affected workflows were rerun against the final implementation. Both new reels pass: `builders` launches a second process to reopen designs after restart, and `status80` exercises validation, failure, save, and delete at 80×24.
- Eighteen distinct real-sqld terminal workflows passed across the full sweep and final reruns, including the same seven affected workflows. The two dbhealth-dependent reels were excluded.

Ordinary Esc/Ctrl-Q discard handling, asset discovery, long-list scrolling, the first-run tutorial, and event-loop latency remain separate follow-ups in #47. These changes preserve preview round trips; they do not add recovery of an unsaved design after quitting. Hosted authentication, concurrent editing, and actual printer submission remain outside this batch's terminal coverage. The long remote URL check uses the HTTP fixture; real-server terminal runs use a loopback URL.

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
python3 tools/demo/remote_uitest.py --bin /path/to/sqld --reels qbe
```

For a disposable embedded sweep with a long database path:

```python
import json, sys, tempfile
from pathlib import Path
sys.path.insert(0, 'tools/demo')
import uitest as ui

root = Path(tempfile.mkdtemp(prefix='pf-p2-', dir='/tmp'))
connection = root / ('long-database-directory-' * 6) / ('project-data-' * 5)
connection.mkdir(parents=True)
ui.DB = str(connection / 'ui.db')
ui.OUT, ui.WORK = str(root / 'casts'), str(root / 'work')
ui.ENV = {'PHOSPHOR_EXT': '', 'PHOSPHOR_TOKEN': ''}
results = {}
for reel in ui.reels():
    if reel.name in {'nav', 'health'}:  # unavailable dbhealth extension
        continue
    ui.seed()
    results[reel.name] = reel.run()
(root / 'results.json').write_text(json.dumps(results, indent=2))
print(root, results)
assert not any(results.values())
```

The existing required CI real-sqld step includes the new builder persistence and failure regressions. Terminal recordings remain in the disposable output directories; the UI sweep itself is still a separate local check.

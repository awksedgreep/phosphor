**Related records and complete output — #33 and #34**

Enter, Add, and Delete now operate on the visibly focused pane. A related-record form keeps the child table, physical row identity, and parent relationship. Its previous/next actions stay within that parent's records, and saving returns to the related pane without moving the master selection. The pane fetches additional windows when navigating beyond its original 2,001-row window.

New child records inherit the parent key, including text keys containing quotes and semicolons. The relationship remains fixed even when a crafted form hides the foreign-key field or a lifecycle script tries to change it. Visible parent fields carry a distinct marker and explain their restriction; the child's full table remains the place to reassign them. Composite parent links and children without an unambiguous row identity refuse related-record editing with an explanation. A loading, deleted, or stale child cannot fall back to editing the parent.

Updates now return the row identity from SQLite's RETURNING result. This also fixes an edge case found while checking child forms: changing an INTEGER PRIMARY KEY must keep subsequent edits on the renumbered record. The embedded backend retains its savepoint and exactly-one-row check. Both backends return the authoritative identity through the worker. Related panes locate that identity again when refreshing after a save, so a different cached row cannot become the selected record merely because its position changed.

Final output no longer uses the interactive 10,000-row query cap. CSV exports consume rows incrementally from one SELECT. Embedded SQLite steps the statement directly; remote output uses the [Hrana 3 HTTP cursor](https://raw.githubusercontent.com/tursodatabase/libsql/main/docs/HRANA_3_SPEC.md), reading rows until the statement finishes. A source's explicit LIMIT still applies. Both paths require one read-only SELECT, preserve trailing comments and semicolons, and reject write-shaped sources.

Reports and labels collect the complete result before laying it out, so grand totals, group subtotals, and mailing runs include every selected row. A failed query produces no partial pager. CSV and pager text files are written to a temporary file beside the destination, flushed, and renamed only after successful completion. Query failures, incomplete remote responses, and file-write errors leave an existing destination intact. The remote reader drains a response after a consumer failure so a caller-owned transaction remains available for rollback; incomplete transport responses block unsafe continuation.

**Validation**

- All 194 Rust suite entries passed with all features and real sqld 0.24.32 enabled. Two optional dbhealth integrations return early because the extension is unavailable.
- Shared output regressions run against embedded SQLite, the HTTP fixture, and real sqld. A 10,005-record fixture verifies CSV row count and last record, every label, grand totals, group counts, explicit LIMIT, trailing comments, and empty-result headers. A late integer-overflow error preserves a previous export. A consumer error inside an existing transaction still permits rollback.
- Shared App/worker regressions run against both backends and real sqld: edit a child while preserving its parent, stay inside the relation while paging, reject a duplicate insert without losing the form, insert a non-tail identity with a hidden parent field, delete the intended child, refuse a deleted or loading row, and continue editing after changing its key.
- Additional regressions cover text parent keys, navigation beyond 2,001 child rows, ambiguous physical identities, composite relationships, visible export success/failure, and an HTTP response that ends before the SELECT completes.
- Formatting, Clippy with all targets/features and warnings denied, generated-manual synchronization, the release build, and both release performance budgets passed.
- All 27 available embedded terminal workflows and 15 real-sqld workflows passed. The existing `rowidentity` assertion was updated to include the table name in the confirmation and rerun on both backends; the other workflows passed in the full sweeps. Both final CSV files contain 10,005 records after the attempted failing replacement, and both saved reports contain `TOTAL (10005 rows)` and the sum `50055015`. Temporary output files were removed. The two dbhealth-dependent reels were excluded.

CSV retrieval and writing are streamed. Report and label layout still retains its data and rendered lines in memory; this batch does not claim bounded memory for arbitrarily large layouts. Hosted authentication, concurrent multi-user editing, and actual printer submission remain outside the end-to-end coverage. The recorded terminal runs use 100×30 and short database paths. The existing long-path status-bar issue (#40) remains open; an initial run with a longer temporary path reproduced it in the delete confirmation. The report/label builder's preview-to-design return path remains tracked separately in #39.

**Reproduce**

```sh
cargo test --all-features --quiet
python3 tools/test_sqld.py
cargo clippy --all-targets --all-features -- -D warnings
cargo fmt --check
cargo run --quiet --bin phosphor -- --manual | diff -u docs/MANUAL.md -
cargo build --release
cargo test --release perf_budget -- --nocapture
python3 tools/demo/remote_uitest.py --bin /path/to/sqld
```

The required CI real-server test now includes these output and related-record regressions. For the embedded terminal sweep, use the temporary-output command in the [data-fix report](2026-09-13-data-fixes/README.md). The new `detailcrud` and `completeoutput` reels exercise the visible workflows. The remote runner includes both new reels, report generation, and relation navigation in its default selection.

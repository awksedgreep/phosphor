**Remote transactions and saved SQL text — #35 and #36**

Remote CSV imports now retain the same server connection from BEGIN through COMMIT or ROLLBACK. Duplicate rows, malformed CSV, and deferred constraint failures roll back the import, including trigger side effects. An import refuses to join a transaction the user already opened. Table-editor schema changes also refuse to run alongside that transaction.

Remote SQL scripts use conditional batch steps, so the first failed statement prevents later statements from running. If a failed batch opened its own transaction, the client rolls it back. An existing caller-owned transaction stays available for explicit recovery. Successful BEGIN, SAVEPOINT, COMMIT, RELEASE, and ROLLBACK requests retain or close the stream according to the server's reported transaction state.

The client follows the rotating connection token and optional server URL from the [Hrana 3 protocol](https://raw.githubusercontent.com/tursodatabase/libsql/main/docs/HRANA_3_SPEC.md). Before using an existing transaction, a server-side condition checks that it is still open; an expired transaction cannot silently turn subsequent writes into independent commits. A lost response can leave the outcome unknown, particularly after COMMIT. In that case, further operations are refused until the database is reopened and inspected; writes are never automatically replayed. Closing an acknowledged commit is best-effort cleanup and cannot turn that commit into a reported write failure.

SQL statement boundaries now use [SQLite's completeness check](https://www.sqlite.org/c3ref/complete.html), with a lexical scan that preserves quoted strings, identifiers, comments, and complete trigger bodies. Saved form definitions, scripts, application-item updates, and preferences use bound values. Apostrophes, semicolons, Unicode, and multiline Lua source round-trip without being interpreted as statement separators. The new `sqltext` terminal reel checks the visible results of quoted text, a multi-statement trigger, and a failed transaction.

**Validation**

- All 186 Rust suite entries passed with all features and real sqld enabled. Two optional dbhealth integrations return early because the extension is unavailable.
- The shared workflow regressions passed against embedded SQLite, the local HTTP fixture, and the official [sqld 0.24.32 release](https://github.com/tursodatabase/libsql/releases/tag/libsql-server-v0.24.32). They cover import rollback, failed commits, transaction ownership, stopping failed batches, bound text including NUL, saved labels/scripts, and schema rollback.
- The real-server test also covers native schema edits preserving CHECK, case-insensitive UNIQUE, indexes, triggers, views, AUTOINCREMENT, foreign-key actions, STRICT, and WITHOUT ROWID. This supplies the real-server validation missing from the previous schema-fix batch.
- HTTP fault tests simulate transaction expiry, a lost statement response, and a lost COMMIT response. They assert that later writes are skipped or refused and that no automatic replay occurs. Dropping a connection with an open transaction also rolls it back. These fault injections use the local fixture; the normal workflows above use real sqld.
- All 11 selected terminal workflows passed against disposable real sqld servers: forms, apps, scripting, data, kiosk, tableeditor, rowidentity, insertidentity, generated, csvrollback, and sqltext. The kiosk run left the remote schema and table contents unchanged.
- All 25 available embedded terminal reels passed at 100×30 with a short database path. The two dbhealth-dependent reels remain excluded. Long-path and smaller-terminal behavior remain separately tracked.
- Formatting, Clippy with all targets/features and warnings denied, generated-manual synchronization, the release build, and both release performance budgets passed.

The new CI step runs the real-server regression using a pinned official release verified against its published SHA-256 checksum. The terminal sweeps remain local checks. Hosted authentication, concurrent editing, server-specific transaction timeouts, and the unavailable dbhealth extension have not received full end-to-end coverage in this batch. Table rebuilds that cannot preserve the full schema remain intentionally refused, as documented in the [previous batch](2026-09-13-protection-fixes.md).

**Reproduce**

```sh
cargo test --all-features --quiet
python3 tools/test_sqld.py
cargo clippy --all-targets --all-features -- -D warnings
cargo fmt --check
cargo run --quiet --bin phosphor -- --manual | diff -u docs/MANUAL.md -
cargo test --release perf_budget -- --nocapture
cargo build --release
python3 tools/demo/remote_uitest.py --bin /path/to/sqld
```

`tools/test_sqld.py` downloads and verifies the pinned server in a temporary directory, starts a fresh database, runs the integration test, and removes the temporary files. Supply `--bin /path/to/sqld` or set `PHOSPHOR_SQLD_BIN` to use an installed server. The terminal runner creates a fresh server for each workflow and leaves recordings, logs, and assertion results under `/tmp/pfr-*`. Use the temporary-output command in the [data-fix report](2026-09-13-data-fixes/README.md) for the embedded sweep; no committed demo recordings need to be replaced.

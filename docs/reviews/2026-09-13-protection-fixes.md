**Database protection fixes — #27 and #28**

Read-only mode is now a database capability established before the application starts. Embedded files open with SQLite's read-only flag; query-only mode also protects scratch and temporary databases. An authorizer prevents SQL from disabling the guard, attaching files, or invoking extension/file-writing helpers. The command bus and worker derive their mode from that connection. Record writes, catalog saves, and schema edits reject writes at the backend boundary. Theme and shimmer changes are marked “session only”; startup preference changes are refused.

Remote read-only SQL runs inside a SELECT subquery, allowing SQLite to reject DML, DDL, ATTACH, and PRAGMA statements independently of their spelling or leading comments. The [sqld statement parser](https://github.com/tursodatabase/libsql/blob/d6c75af6353bb1c34985399608e37cd272a35aa1/libsql-server/src/hrana/stmt.rs#L77) rejects multiple statements in one execute request, preventing an escape into a SQL script. Rejected queries are not retried as raw SQL. Metadata remains available through `pragma_*` table functions. The remote path does not set `query_only`: [sqld's query policy](https://github.com/tursodatabase/libsql/blob/d6c75af6353bb1c34985399608e37cd272a35aa1/libsql-server/src/query_analysis.rs#L147) rejects that connection setting. Server-installed SQL functions remain trusted code; server credentials determine permissions outside this application's SQL paths.

The table editor now uses SQLite's native ALTER operations. It refuses type, constraint, and existing-column order changes that require a rebuild because its column model cannot represent the complete original schema. A refusal leaves the draft open and explains that a SQL migration is needed. Fields retain their original identity during edits, preventing a combined rename/type change from becoming a destructive drop/add. New columns must follow existing columns, matching SQLite's append behavior. The preview no longer repeats table-renaming statements.

Both backends apply the generated ALTER statements in one transaction and check foreign keys before committing. Embedded edits refuse to take over a caller's transaction or use legacy ALTER behavior. Remote edits use [Hrana conditional batch steps](https://raw.githubusercontent.com/tursodatabase/libsql/main/docs/HRANA_3_SPEC.md): a failed ALTER or foreign-key check skips subsequent changes and COMMIT, and runs ROLLBACK. A conditional SQL expression raises an arithmetic error only when foreign-key violations exist, making validation fail before commit without temporary tables, which sqld forbids. The client reports this as a foreign-key check failure. Neither backend reconstructs the table or discards its triggers and constraints.

**Validation**

- All 178 Rust suite entries passed with all features enabled. Nine new tests cover database protection, saved-query writes, rejected remote reads, schema dependencies, and transaction failures. Three existing optional integrations return early because real sqld and the dbhealth extension are unavailable.
- Formatting, Clippy with all targets/features and warnings denied, generated-manual synchronization, and the release build passed.
- Both release performance budgets passed: approximately 25.3 µs per embedded page and 8.7 µs per worker round trip.
- All 24 available terminal reels passed at 100×30 with a short database path. The kiosk database remained byte-for-byte unchanged. The two dbhealth-dependent reels were excluded; long-path and smaller-terminal issues remain separately tracked.
- Embedded read-only regressions compare the database file byte for byte after write attempts and preference changes. Remote HTTP tests run in the normal suite using the [local SQLite-backed fixture](../../src/test_support.rs); they require no external service.
- Schema tests preserve CHECK, case-insensitive UNIQUE, explicit indexes, triggers, views, AUTOINCREMENT state, inbound references, FK actions, STRICT, and WITHOUT ROWID. They also check rollback after a later ALTER fails, after an FK check fails, and when a caller already owns an embedded transaction.

General remote transaction ownership and free-form SQL splitting remain tracked in #35 and #36. These changes provide a dedicated atomic path for table-editor changes; they do not yet fix remote CSV imports or arbitrary SQL scripts. The local fixture exercises the client's requests and SQLite behavior; real sqld validation remains outstanding.

**Reproduce**

```sh
cargo test --all-features --quiet
cargo clippy --all-targets --all-features -- -D warnings
cargo fmt --check
cargo test --release perf_budget -- --nocapture
cargo build --release
```

For the terminal sweep, use the temporary-output command in the [previous validation report](2026-09-13-data-fixes/README.md). The `kiosk` reel now tries a CTE write and a session-only preference change; `tableeditor` exercises a refused rebuild and retains its draft. The `nav` and `health` reels require the optional dbhealth extension.

**Data integrity fixes from the 13 September review**

This batch addresses [#30](https://github.com/awksedgreep/phosphor/issues/30), [#31](https://github.com/awksedgreep/phosphor/issues/31), [#32](https://github.com/awksedgreep/phosphor/issues/32), and [#37](https://github.com/awksedgreep/phosphor/issues/37). The remaining work is tracked in [#47](https://github.com/awksedgreep/phosphor/issues/47).

- **Record identity:** both backends resolve an unshadowed SQLite identity and use it consistently for paging, edits, deletion, and computed-field lookups. Write predicates reject an alias that was subsequently shadowed. Embedded row writes also roll back trigger effects when the expected-one-row check fails. Views, virtual tables, WITHOUT ROWID tables, and tables declaring all three aliases are explicitly read-only in the record UI.
- **Inserted records:** the form reloads the identity returned by INSERT. It finds that identity again when fetching neighboring rows, so an explicit ID, gaps, trigger activity, or a competing insert cannot substitute the last record. If a trigger removes the inserted row, the user receives an error and the form retains that identity without inserting duplicates or switching to another row.
- **Generated columns:** both backends now return metadata in the same order as `SELECT *`, including generated columns and excluding hidden virtual-table arguments. Generated values stay read-only, navigation skips them, and saves reload their computed values. CSV imports explain that generated columns must be omitted. The table designer refuses structure edits that would lose generation expressions; the broader schema-preservation work remains in #28.
- **Failed imports:** embedded imports require a successful BEGIN, roll back parse and insert errors, and propagate commit failures. They refuse to join or finish an existing transaction. The terminal regression also caught and fixed stale affected-row counts for BEGIN, ROLLBACK, and DDL.

**Validation**

- `cargo test --all-features --quiet`: 169 suite entries passed, including 11 new tests. Three existing optional integration tests return early because real sqld and the dbhealth extension are unavailable.
- Formatting, Clippy with all targets/features and warnings denied, generated-manual synchronization, and release build passed.
- Both release performance budgets passed: approximately 25.4 µs per embedded page and 8.5 µs per worker round trip in the measured run.
- All 24 available terminal reels passed at 100×30 with a short database path. This includes four new regressions: `rowidentity`, `insertidentity`, `generated`, and `csvrollback`. The two extension-dependent reels (`nav` and `health`) were excluded. Long-path and smaller-terminal problems remain separate open issues.
- Two isolated remote probes passed against the local SQLite-backed Hrana fixture. They exercise alias selection, safe edit/delete, generated metadata, and a form insert with ID 50. The fixture inserts ID 25 between the saved-row lookup and its next page fetch; the form stays on ID 50 and updates its grid position correctly.

The remote fixture validates the client's SQL and protocol behavior; it does not establish real sqld transaction correctness. The remote transaction issue #35 remains open, and the CSV rollback fix here concerns embedded transaction ownership.

**Reproduce**

Run the Rust checks above from the repository root. The [remote runner](run_remote_probes.py) creates a temporary source/database copy, starts a local fixture on an ephemeral port, runs [the probes](remote_probes.rs), stops the fixture, and prints the artifact directory:

```sh
python3 docs/reviews/2026-09-13-data-fixes/run_remote_probes.py
```

To run the terminal sweep without replacing committed demo recordings:

```sh
cargo build --release
python3 -u - <<'PY'
import os, sys, tempfile
sys.path.insert(0, 'tools/demo')
import uitest as ui
root = tempfile.mkdtemp(prefix='pf-', dir='/tmp')
ui.DB, ui.OUT, ui.WORK = [os.path.join(root, n) for n in ('ui.db', 'casts', 'work')]
ui.ENV = {'PHOSPHOR_EXT': ''}
failed = False
for reel in ui.reels():
    if reel.name in {'nav', 'health'}:
        continue
    ui.seed()
    errors = reel.run()
    print(reel.name, 'FAIL' if errors else 'PASS', errors)
    failed |= bool(errors)
print('Recordings:', root)
sys.exit(failed)
PY
```

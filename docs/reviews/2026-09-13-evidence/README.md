**Review evidence — 13 September 2026**

These files support [the code and user experience review](../2026-09-13-code-and-ux.md). Production source was not modified. Diagnostic tests were added only to a disposable copy of the repository and exercised synthetic data.

| Artifact | Meaning |
|---|---|
| [validation.txt](validation.txt) | Baseline checks, optional integration limits, and performance results |
| [review_probes.rs](review_probes.rs) | 20 local and 3 remote diagnostic checks asserting desired behavior |
| [local-probes.log](local-probes.log) | All 20 local checks failed with the reported observed behavior |
| [remote-probes.log](remote-probes.log) | Three remote checks failed against the protocol fixture |
| [hrana_fixture.py](hrana_fixture.py) | Local SQLite-backed HTTP fixture used to inspect client transaction and SQL behavior |
| [ui-short-path.log](ui-short-path.log) | Existing PTY sweep: 18 passed, 2 failed due to missing dbhealth fixtures |
| [ui-long-path.log](ui-long-path.log) | Same sweep with a long database path: 2 passed, 18 failed, mostly because status messages were clipped |
| [first-run-80x24.txt](first-run-80x24.txt) | Reconstructed first-run terminal screen |
| [hidden-validation-error-100x30.txt](hidden-validation-error-100x30.txt) | Long-path session where a required-field error was absent from the visible status bar |
| [query-display-latency-ms.json](query-display-latency-ms.json) | Five input-to-visible-query-header observations in milliseconds |

The diagnostic failures are intentional evidence of missing guarantees. They are separate from the existing 149-test suite, which passed. The remote fixture implements independent execute requests and closes the SQLite connection on the client's close request. It is not a replacement for real sqld integration testing.

**Reproduce the diagnostics in a fresh isolated copy**

Run these commands from the repository root. They create temporary files and synthetic databases; a sandbox probe writes and then removes a harmless temporary text file. Use a new temporary copy for each complete run, since the remote tests create fixture tables.

```sh
review_dir="$(mktemp -d)"
cp Cargo.toml Cargo.lock "$review_dir/"
cp -R src "$review_dir/src"
mkdir "$review_dir/docs"
cp docs/MANUAL.md "$review_dir/docs/"
cp docs/reviews/2026-09-13-evidence/review_probes.rs "$review_dir/src/review_tests.rs"
printf '\n#[cfg(test)]\nmod review_tests;\n' >> "$review_dir/src/main.rs"
printf '%s' "$review_dir" > /tmp/phosphor-review-location
```

In a second terminal at the repository root, start the local fixture. Stop it with Ctrl-C after the checks complete:

```sh
python3 docs/reviews/2026-09-13-evidence/hrana_fixture.py
```

In the original terminal, run the isolated diagnostic module:

```sh
env -u PHOSPHOR_EXT -u PHOSPHOR_TOKEN \
  CARGO_TARGET_DIR="$PWD/target" \
  cargo test --manifest-path "$review_dir/Cargo.toml" review_tests -- --nocapture
```

Expect a failing command on the reviewed revision: the assertions describe the intended fixed behavior. The standard tests are filtered out. The preserved logs are from a 20-local-check run followed by a separate 3-remote-check run.

**Interpret the UI sweeps**

The review imported `tools/demo/uitest.py`, redirected `DB`, `OUT`, and `WORK` to temporary locations, and disabled the unavailable extension. The existing test data and input sequences were otherwise retained. With a short `/tmp/.../ui.db` path, all 18 extension-independent reels passed. The navigation reel assumes a dbhealth view exists, and the health reel assumes the extension is installed; those two failed on this machine. This is an environment limitation rather than evidence that ordinary navigation is broken.

With a long macOS temporary-directory database path, status confirmations and errors were hidden behind the connection name. Eighteen reels then failed, while the applications-generator and app-mode reels passed. Read the log alongside the saved validation-error screen; many failures are repeated manifestations of one layout defect, not separate functional failures.

**Interpret the timing observations**

Five statements of the form `SELECT 42 AS probe_N` were submitted through a real PTY to the release binary using its in-memory database. The recorder compared the input schedule to the first reconstructed screen containing the corresponding result-column header, not to text typed in the prompt. Its polling interval contributes approximately 20 ms of scheduling granularity. Results of 252.1–268.8 ms align with the application's 250 ms terminal-event poll. They should not be treated as a statistically rigorous benchmark, but they establish an interaction delay that the backend-only performance tests miss.

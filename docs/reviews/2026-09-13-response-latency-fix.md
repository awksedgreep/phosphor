**Prompt response and cancellable output — #46**

The event loop previously waited up to 250 ms for a terminal event before collecting a database reply. A reply that arrived just after the input handler could therefore sit unnoticed for a quarter second. It now collects replies before drawing and waits at most 8 ms while jobs are pending. Static idle screens retain the 250 ms wait and dirty-only redraws. A required viewport repaint uses a zero wait. Keyboard, mouse, and resize events share the 64-event processing budget, so a stream of mouse or resize events also yields to database replies and drawing.

The real-terminal measurement distinguishes fast queries that happen to finish during input handling from those that finish just afterward. The latter used to show the consistent delay. Measurements on this macOS development machine, using release binaries and an 80×24 terminal:

| Probe | Before | After |
|---|---|---|
| `SELECT 42` | Often immediate; an initial ten-query run included 250–251 ms stalls | Ten-query runs completed within 10 ms |
| Recursive sum over 20,000 values, five queries | 250.9–252.9 ms | 8.4–9.5 ms |
| Real sqld through a proxy adding 25 ms to probe requests | Not measured | SELECT: 28.4–39.9 ms; recursive sum: 37.8–48.2 ms |

These measure Enter to the visible result header, including terminal rendering and transport. They are not backend microbenchmarks or guarantees for arbitrary query execution. Existing backend and worker performance budgets remain in place.

Reports, labels, and CSV exports now run as complete worker jobs, including data collection and report layout. A progress screen shows rows read and elapsed time. Help, full status details, terminal resizing, and Ctrl-Q remain available. Esc requests cancellation; the previous screen remains protected until the worker acknowledges completion. Other database actions are held during this work, so a synchronous command cannot queue behind it and freeze cancellation. Older read continuations that would synchronously fetch preferences are deferred until the output job releases the worker.

Embedded jobs install a SQLite progress callback while their read runs, allowing cancellation during computation before the first row. Collection and layout also check cancellation. Remote autocommit output closes its own stream after a cancelled or failed consumer; reads inside a caller-owned transaction drain the response to retain the transaction and its continuation. Remote cancellation may therefore wait for a response or network timeout. A transport failure that leaves transaction ownership uncertain remains an error, even if the user also requested cancellation.

In the slow-operation terminal checks, Help and status details responded in under 1 ms on both backends. With a 600 ms proxy delay, remote cancellation acknowledged in 612–773 ms while the UI remained available; quit took about 23 ms. The local checks cancelled an aggregate over a billion-value recursive source before its first result, plus streaming CSV output, without replacing the existing export. Local cancel acknowledgments were under 11 ms and quit took about 30 ms in the recorded run. The tests also resize through 40×12 and verify failure and successful retry.

CSV publication has a cancellation boundary: cancellation before publication prevents replacement; once publication begins, the UI reports that it is finishing. Failed and cancelled exports remove their temporary output and preserve an existing destination. Report failure/cancellation restores the design; success still supports Esc back to the same report or menu. Help can remain open while a job completes and reveal the finished preview when closed.

Worker shutdown now resolves every outstanding async token as a failure, preventing a permanent pending/progress state after a worker panic. Tests also cover queued jobs and an older read arriving while output occupies the worker.

**Validation**

- All 219 Rust suite entries passed with all features and official sqld 0.24.32 enabled. Two optional dbhealth integrations return early because the extension is unavailable.
- Cancellation during a streamed read preserves an existing transaction and allows its explicit rollback on embedded SQLite, the Hrana fixture, and real sqld. A lost cursor reply reports unknown transaction ownership and prevents further requests from replaying work.
- All 34 available embedded terminal workflows and 11 selected real-sqld workflows passed, covering reports, forms, menus, read-only mode, complete output, builder returns, visible status, resizing, SQL previews, and the complete CRM walkthrough/restart.
- The new local PTY check enforces Enter-to-visible-result budgets and responsiveness during slow output, checks failure recovery, preserves an existing export on cancel/error, validates successful retry, and times quit during work. CI now runs it after the release build.
- A separate delayed-proxy check runs the same output journeys against a real sqld server. It records query/Help/status/quit response times separately from cancellation acknowledgment.
- Formatting, Clippy with all targets/features and warnings denied, generated-manual synchronization, release build, and both release performance budgets passed.

The full terminal sweep and delayed-server check remain local checks; CI runs the new local latency/cancellation probe and the existing required real-sqld Rust regression. Record/schema writes, imports, scripts, printer calls, saved-database backup, and some metadata loads still use synchronous calls. These remaining paths need separate progress/cancellation work and are retained in #47. Report/label layouts still retain complete results in memory. Saved-asset discovery remains in #45, and optional dbhealth/printer/telemetry setup remains outside this validation.

**Reproduce**

```sh
cargo build --release
python3 tools/test_latency.py
python3 tools/test_latency.py --binary /path/to/baseline/phosphor --measure-only
python3 tools/test_latency_remote.py --sqld-bin /path/to/sqld
cargo test --all-features --quiet
python3 tools/test_sqld.py --bin /path/to/sqld
cargo clippy --all-targets --all-features -- -D warnings
cargo fmt --check
cargo run --quiet --bin phosphor -- --manual | diff -u docs/MANUAL.md -
cargo test --release perf_budget
python3 tools/demo/remote_uitest.py --bin /path/to/sqld --reels reports forms apps kiosk appmode completeoutput builders status80 scrolling querysyntax tutorial
```

Latency tools use disposable directories and print their artifact paths. Each retains the reconstructed terminal recording and JSON measurements. The local budget is a 75 ms median and 200 ms maximum for simple query responses, 200 ms for each calculation and local progress/Help/cancel/quit response. The delayed-server check keeps the 200 ms interaction budget and allows two seconds for remote cancellation acknowledgment.

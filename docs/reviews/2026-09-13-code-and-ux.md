**Phosphor code and user experience review — 13 September 2026**

Reviewed commit: `ba364aa`, with a clean working tree at the start. This review covers the application code, database backends, builders, scripting, documentation, tests, and live terminal workflows. Production code was not changed. Reproductions use disposable databases and an isolated copy of the source.

**Verdict: the core product exists, but users cannot yet reliably take full advantage of its promise.** The demonstrated small CRM workflow works in many places. Building, revising, and operating an application introduces gaps that the demos do not exercise. Several defects can change the wrong data, weaken database constraints, lose unsaved work, or produce incomplete output. Those should precede further feature expansion.

The product's strongest qualities are worth preserving: one portable database contains data and application definitions; a keyboard command bus makes behavior testable; QBE exposes its SQL; forms, report previews, menus, memo editing, and themes are implemented; embedded paging is fast. The remaining work is largely about trustworthy state transitions, complete data operations, and connecting those features into a discoverable workflow.

**Evidence and limits**

- `cargo test --all-features`: **149 passed**. Three optional integration tests return early when the real sqld binary or dbhealth extension is missing; those dependencies were unavailable here.
- Release build, `cargo fmt --check`, and Clippy with all targets/features and warnings denied: **passed**.
- Release performance tests: **2 passed**; approximately **30.3 µs** per embedded page and **9.2 µs** per worker round trip in this run.
- Twenty additional local diagnostic checks reproduced the defects described below. These assert desired behavior and deliberately fail against the current implementation; they were run in a temporary source copy, outside the normal suite.
- Three additional remote checks reproduced SQL splitting and transaction problems against a local SQLite-backed Hrana protocol fixture. This verifies the client's request behavior, not a production sqld deployment.
- Live terminal sessions covered all 20 existing UI reels, a fresh 80×24 terminal, long database paths, and query display latency. With a short path, 18 reels passed; two failed because they assume unavailable dbhealth fixtures. With a long path, 18 failed, mostly on hidden status messages. Detailed results are in the evidence directory.
- This is an engineering and heuristic usability review, not a study with representative nontechnical users. Real printer hardware, live sqld, and the optional telemetry extension remain unverified.

Reproduction source, logs, the first-run screen, and timing observations are retained in [the evidence directory](2026-09-13-evidence/README.md).

**Prioritized findings**

P1 means fix before relying on the affected feature for real business data or distributing applications to others. P2 means a material obstacle to completing the intended workflow.

1. **[P1] Lua's advertised sandbox permits host filesystem access.** [src/script.rs:254](../../src/script.rs#L254)

   `engine()` uses `Lua::new()` and sets heap/instruction limits, but does not restrict the loaded standard libraries. A script using `io.open()` successfully wrote a harmless temporary file during this review. Database files can carry lifecycle scripts that run during editing, so copying an application can also introduce host-side behavior. Restrict the available libraries and remove filesystem, process, and dynamic-loading entry points. Resource limits are useful but do not establish the advertised capability boundary. Acceptance: menu and lifecycle scripts retain the documented database/UI helpers while attempts to access host files or launch processes fail.

2. **[P1] Read-only mode is enforced by command shape, not by the database.** [src/app.rs:5532](../../src/app.rs#L5532), [src/db.rs:752](../../src/db.rs#L752), [src/store.rs:45](../../src/store.rs#L45)

   The prompt treats any statement beginning with `WITH` as a query. A CTE-prefixed INSERT passed through that route and inserted a row with `readonly = true`. The proof also avoids the unrelated LIMIT rewriting problem. Separately, `set theme amber` created metadata tables and wrote preferences in a read-only session. The underlying connection is opened for writes. Enforce read-only operation at the backend boundary, with read-only connection/authentication settings where available, and keep session preferences out of the protected database. Acceptance: every entry path, including saved queries, preferences, and commands, leaves a kiosk database unchanged.

3. **[P1] Table rebuilds discard constraints and triggers.** [src/creator.rs:191](../../src/creator.rs#L191), [src/creator.rs:448](../../src/creator.rs#L448), [src/app.rs:3233](../../src/app.rs#L3233)

   The reconstructed schema includes a limited column model, while the caller captures only explicit index SQL. Changing a column type on a table with UNIQUE, CHECK, and an INSERT trigger removed all three behaviors: duplicate names and negative values became accepted, and the trigger disappeared. The same representation cannot preserve all foreign-key actions or table options. Capture and preserve the complete dependent schema, or refuse transformations whose semantics cannot be retained. Check referential integrity before committing and inspect the result. Acceptance: a schema change preserves rows, constraints, triggers, indexes, and relationships, with a complete rollback on failure.

4. **[P1] An unchanged text default can trigger a rebuild and change future values.** [src/creator.rs:117](../../src/creator.rs#L117), [src/creator.rs:191](../../src/creator.rs#L191)

   Introspection stores `DEFAULT 'new'` as SQL text. The designer feeds that SQL text through a routine that quotes user-entered text again. Opening this table and compiling an unchanged draft generated a full rebuild with `DEFAULT '''new'''`, so the new default includes literal apostrophes. Keep raw SQL expressions distinct from user-entered values and compare their intended representations. Acceptance: opening and applying an untouched table is a no-op, including quoted text, expression, and numeric defaults.

5. **[P1] A user-defined `rowid` column can cause one edit to modify multiple records.** [src/db.rs:653](../../src/db.rs#L653), [src/db.rs:802](../../src/db.rs#L802)

   Row identity is hardcoded as the identifier `rowid`. SQLite permits a real column with that name. In a fixture containing two rows with a user-defined `rowid` of 7, updating the displayed identity changed both records. The function reported an expected-one-row error after the mutation had happened. Resolve an unshadowed identity or a complete primary key; keep multirow effects transactional so a failed cardinality check rolls back. Acceptance: editing or deleting one displayed record never changes another, including tables with shadowed rowid aliases and composite keys.

6. **[P1] Saving a non-tail insert switches the form to an unrelated record.** [src/app.rs:5175](../../src/app.rs#L5175), [src/app.rs:4307](../../src/app.rs#L4307)

   After insertion, the form reloads the last positional record rather than locating the returned identity. Inserting ID 50 into a table containing IDs 1 and 100 switched the editor to ID 100. Further typing would update that existing record. Reopen by the returned key and reconcile the cursor position separately. Acceptance: explicit IDs, gaps, trigger activity, and concurrent inserts cannot change which record remains open after Save.

7. **[P1] Generated columns shift values onto the wrong field names.** [src/db.rs:620](../../src/db.rs#L620), [src/app.rs:4398](../../src/app.rs#L4398)

   Column metadata comes from `table_info`, while row values come from `SELECT *`. Generated columns appear in the latter but are absent from the former. The editor zips the two lists by position. For `(id, generated_size, name)`, the field labelled `name` displayed integer 5 instead of `Alice`. Reuse explicit, consistent projections or richer column introspection, and represent generated columns as read-only. Acceptance: every displayed name/value pair remains correct with generated columns at any position, and saves cannot write generated values into ordinary fields.

8. **[P1] Enter in the focused detail pane opens the parent editor.** [src/app.rs:1574](../../src/app.rs#L1574), [src/app.rs:4270](../../src/app.rs#L4270)

   The detail keymap maps Enter to `OpenEdit`, but `open_edit()` always reads `self.grid`, the master grid. With an order selected in a customer/order split, Enter opened the customer. This creates a risk of editing the wrong record and makes related data awkward to operate. Route editing through the focused pane and retain a stable child identity. Until supported, explicitly explain the limitation instead of opening another record. Acceptance: Enter, Add, and Delete act on the visibly focused table, with the parent relationship retained.

9. **[P1] Export and label output silently stop at 10,000 rows.** [src/csv_io.rs:126](../../src/csv_io.rs#L126), [src/report.rs:351](../../src/report.rs#L351), [src/db.rs:752](../../src/db.rs#L752)

   These operations use the same capped query API as interactive browsing and ignore `truncated`. Exporting a 10,001-row fixture produced 10,000 data rows and a success message. Labels also ignore truncation. Banded reports disclose truncation only after emitting partial totals at the end. Introduce streaming or paginated complete-output operations, and clearly distinguish previews from final output. Acceptance: an export, mailing run, or business total includes every selected record, or fails explicitly before presenting a completed artifact.

10. **[P1] Remote transactions do not provide the rollback behavior callers expect.** [src/remote.rs:58](../../src/remote.rs#L58), [src/csv_io.rs:84](../../src/csv_io.rs#L84)

    Every pipeline ends with `close`, with no retained baton. CSV's BEGIN, row writes, and COMMIT therefore use separate sessions. A duplicate-key failure left the first imported row committed in the protocol fixture. Within one pipeline, independent execute requests also continued through COMMIT after an earlier statement failed. That request pattern is especially dangerous for a table rebuild that subsequently drops the original table. Add an explicit transaction API and use retained sessions or conditional Hrana batches that gate commit and later mutations on success. Acceptance: a mid-import or mid-rebuild error leaves the remote database exactly as it was before the operation. Verify this against real sqld before claiming backend parity.

11. **[P1] Remote SQL splitting breaks ordinary text and stored scripts.** [src/remote.rs:372](../../src/remote.rs#L372)

    `execute()` splits on every semicolon without respecting SQL strings or comments. `INSERT ... VALUES ('Ada; Grace')` failed against the fixture. Form definitions, menu targets, and Lua source are stored through SQL text, so semicolons in those values encounter the same problem. This is broader than manually entered multi-statement SQL. Accept structured statement lists for batches and bind values for application storage. Acceptance: semicolons, quotes, multiline scripts, and trigger bodies round-trip over both backends.

12. **[P1] A CSV parse error leaves an embedded transaction open with partial data.** [src/csv_io.rs:84](../../src/csv_io.rs#L84)

    Insert failures trigger rollback, but a malformed CSV record exits via `?` before that cleanup. A valid first row followed by a short second row returned an error while leaving the first row visible inside the open transaction. BEGIN and COMMIT failures are also ignored. Subsequent work can unexpectedly join or commit that transaction. Use transaction ownership with rollback on every error path and propagate begin/commit errors. Acceptance: malformed input, constraint failures, existing transactions, and commit failures cannot produce a success message or retain a partial import.

13. **[P1] Opening Help destroys the screen the user needs help with.** [src/app.rs:2313](../../src/app.rs#L2313), [src/app.rs:2214](../../src/app.rs#L2214)

    F1 replaces the sole overlay with Help; Esc then replaces Help with None. It does not restore the table draft, form designer, report, or script editor. A newly named unsaved table draft was lost after F1 → Esc. This directly undermines learning by exploration. Preserve the suspended screen and its draft, cursor, and scroll state in a navigation stack. Acceptance: F1 and Esc are a lossless round trip from every screen, including unsaved scripts and memo editing. Review ordinary Esc and Quit paths for explicit handling of dirty drafts as well.

14. **[P2] Running a builder destroys the design before the user can revise or save it.** [src/app.rs:3192](../../src/app.rs#L3192), [src/app.rs:2636](../../src/app.rs#L2636), [src/qbe.rs:247](../../src/qbe.rs#L247)

    Report preview replaces the designer; Esc returns to the browser, losing an unsaved title/source/grouping. QBE Run also replaces its designer; pressing Q afterward starts from empty filters. The saved QBE JSON is written but has no corresponding designer loader, so saved queries can run but cannot be reopened graphically for maintenance. The CRM guide's preview → Esc → F6 report-saving sequence consequently does not work as written. Retain a design/result navigation history, add saved-query loading, and make Save, Save As, Rename, and overwrite behavior explicit. Acceptance: create → run → revise → save → restart → reopen works for every builder.

15. **[P2] Database paths can hide every error and confirmation.** [src/ui.rs:1176](../../src/ui.rs#L1176)

    The status bar prints the full database path before the message and reserves the right edge for row/latency information. A sufficiently long path leaves zero visible message space. Live sessions hid required-field errors, successful saves, and delete-confirmation text; 18 of the 20 existing reels failed under this path layout, mostly on invisible messages. Prioritize errors and outcomes; abbreviate the path to a basename and expose connection details separately. Acceptance: validation, save failures, and destructive-action prompts remain readable at 80 columns with long local paths and remote URLs.

16. **[P2] Lists and forms allow the selection to move offscreen without scrolling.** [src/ui.rs:902](../../src/ui.rs#L902), [src/ui.rs:1299](../../src/ui.rs#L1299), [src/ui.rs:399](../../src/ui.rs#L399), [src/ui.rs:603](../../src/ui.rs#L603), [src/ui.rs:542](../../src/ui.rs#L542)

    The sidebar renders a stateless List; several builders and the record form render unscrolled Paragraphs. The selected 36th table and a field near the end of a 35-field form were invisible at 80×24. The same rendering pattern affects long menus and builder lists. Preserve a viewport offset tied to the selection, support page navigation, and keep the caret visible horizontally. Acceptance: every item and editable field remains reachable and visibly selected across terminal sizes and resizing. Also check help wrapping and long script lines, which currently clip horizontally.

17. **[P2] Foreign-key selection ends at the first 200 parent records.** [src/app.rs:4481](../../src/app.rs#L4481)

    The picker has a fixed `LIMIT 200` and supports only moving through the fetched list. It has no search, fetch-next-page, or truncation message. Customer 201 in a 201-customer fixture was unavailable. Typing an ID manually assumes knowledge the picker is meant to remove. Add searching by useful display fields and key, real paging, a result count, and an optional create-related-record flow. Acceptance: a nontechnical user can associate an order with any customer in a large table without memorizing database IDs.

18. **[P2] Generic LIMIT rewriting invalidates supported SQL.** [src/db.rs:414](../../src/db.rs#L414), [src/db.rs:752](../../src/db.rs#L752), [src/app.rs:5532](../../src/app.rs#L5532)

    All queries without the substring `limit` receive an appended LIMIT. The prompt explicitly routes PRAGMA and VALUES through this API, but `PRAGMA table_info(t)` becomes invalid SQL. A substring in a comment or literal also changes whether the rewrite happens. Keep SQL execution semantics separate from display caps, using statement-aware handling or result iteration limits where necessary. Acceptance: documented PRAGMA, VALUES, EXPLAIN, CTE, and commented SQL work unchanged, while large interactive results remain bounded.

19. **[P2] The first-run path and CRM guide do not get a new user to their first record reliably.** [src/ui.rs:930](../../src/ui.rs#L930), [src/ui.rs:989](../../src/ui.rs#L989), [src/app.rs:4800](../../src/app.rs#L4800), [docs/BUILD-A-CRM.md:58](../BUILD-A-CRM.md#L58)

    A fresh screen offers Enter on a nonexistent table, the SQL prompt, Help, and splitting; it does not surface Create Table as the next step. The default scratch session is identified only as `:memory:` and has no prominent explanation of its lifetime or guided Save Database action. The guide says to type Ada immediately after Add, but the uncrafted form starts on `id`; following those keys produced a datatype-mismatch error. Start new record entry on the first appropriate user field, make generated identities clear, add a visible empty-state creation action, and test the literal guide against a pristine database. Explain persistence before the user builds an application in scratch memory.

20. **[P2] Saved assets are difficult to discover and wire into an application.** [src/ui.rs:902](../../src/ui.rs#L902), [src/app.rs:3603](../../src/app.rs#L3603), [src/appsgen.rs:16](../../src/appsgen.rs#L16), [src/app.rs:3080](../../src/app.rs#L3080)

    The README's Data/Queries/Forms/Reports/Apps/Admin home does not exist as a navigable catalog. The real sidebar lists data; A opens the first app's designer, and menu targets are free text. Users must remember asset names and command syntax. The menu action enum has no declarative Open Form, New Record, or Submenu action. Report renaming deletes the old catalog row without updating menu references. Add an asset browser with type-aware target selection, reference validation, direct form/new-record actions, and dependency-aware renames. Acceptance: after restarting, a user can find, edit, preview, and attach each saved asset without SQL or remembered names, and broken menu targets are caught before handoff.

21. **[P2] The main loop adds roughly a quarter-second to fast query results.** [src/main.rs:185](../../src/main.rs#L185), [src/main.rs:220](../../src/main.rs#L220), [src/worker.rs:222](../../src/worker.rs#L222)

    Database replies do not wake the terminal event poll. Five live scratch-database `SELECT 42` queries took approximately 252–269 ms from scripted input to visible result headers, consistent with the 250 ms poll timeout. Recorder scheduling contributes some measurement overhead, but cannot explain the gap from the microsecond backend timings. Many other operations still use blocking worker calls, so large exports, reports, or slow servers can stall input. Wake on worker completion or use a short pending-work poll, move expensive operations off the input path, and expose progress/cancellation. Acceptance: measure input-to-visible-result latency and responsiveness during slow operations, alongside backend microbenchmarks.

**How well the user journeys work today**

| User's goal | What is present | What prevents full use |
|---|---|---|
| Start a persistent application | File creation, table designer, tutorial | Empty-state guidance, scratch persistence, incorrect initial record-entry instructions |
| Maintain business records | Browse, CRUD, memo editor, typed values | Row identity defects, generated-column alignment, hidden errors, long forms |
| Work with related data | FK discovery, linked panes, value picker | Detail editing selects the parent; picker stops at 200; linked query views have limited editing |
| Build and maintain queries | Filters, projection, sort, one FK join, grouping, save/run | Run discards the design; saved designs cannot reload; no visible asset catalog |
| Design entry screens | Labels, required fields, masks, computed fields, painter | Help/draft loss, clipping, weak direct menu access to record creation/forms |
| Produce business output | Banded reports, totals, text files, printer command, labels | Preview state loss, incomplete large outputs; labels have fixed geometry and include raw columns |
| Hand an application to a team | Stored menus, hotkeys, `--app`, boot preference | Free-text target wiring, reference maintenance, missing form/submenu actions, read-only bypasses |
| Use the network backend | Hrana transport and typed values | Transaction lifecycle and SQL splitting; real deployment not validated in this review |
| Add business logic | Lua helpers and lifecycle events | Host capability exposure; lifecycle effects and event semantics need stronger integration checks |
| Diagnose database health | Console, sampling paths, advisory UI | Optional extension unavailable here; setup needs a clear capability-aware path |
| Enjoy terminal responsiveness | Fast paging, dirty rendering, worker thread | 250 ms result-delivery delay and remaining blocking operations |

**Code quality and coverage implications**

The command bus, backend trait, common value type, pure report layout, and PTY replay tools provide a useful foundation for fixes. The largest structural issue is that these abstractions do not yet carry all their promised guarantees. A method named `query` can write and truncates data; there is no transaction scope in `DbLink`; the single overlay cannot express returning to a suspended editor; record identity is mixed with positional paging. Fix those contracts first, then simplify callers.

`app.rs` contains roughly 6,050 lines before its main test module and over 8,700 lines in total. Separate navigation/draft ownership, record operations, builder controllers, and worker reconciliation along their existing responsibilities. This would reduce duplicated guards and make complete workflow tests easier to write. Do this alongside the contract fixes rather than as an isolated reorganization.

Error propagation also needs attention. Menu label/target edits ignore the result of `update_item`; preference saves report remembered settings while discarding storage errors; several catalog readers turn database failures into an empty catalog. Return failures explicitly and keep the last known valid state so a server outage cannot look like a missing application or successful save.

Additional source-level gaps deserve focused follow-up: lifecycle scripts return queued UI effects that `run_edit_script`/`run_save_script` do not dispatch; `OnSave` observes the post-insert form after `inserting` has been cleared; all users share the fixed preference identity `me`; and updates lack an optimistic concurrency check for another user's edits. These were identified by inspection, not included in the 23 diagnostic reproductions.

The existing test suite is strongest on small, known schemas and isolated command flows. CI does not run the PTY sweep; optional integrations silently return success when unavailable. Performance gates cover page and worker timings, but not frame rendering, startup, or input-to-result delivery. Expand coverage with real sqld transactions, schema preservation, complete output, long paths, 80×24 resizing, non-ASCII text, failed saves, and navigating away from unsaved work. Test written tutorial keystrokes themselves; a different successful demo sequence does not validate the guide.

**Recommended delivery order**

1. Establish trustworthy boundaries: restrict Lua capabilities, enforce backend read-only access, preserve schema and transaction atomicity, and fix record identity/alignment. Prevent incomplete exports and labels.
2. Make editing and learning dependable: preserve drafts across Help and Preview, act on the focused pane, keep errors and selections visible, and remove the picker ceiling.
3. Complete the application-building loop: asset catalog, saved QBE editing, typed menu targets, reference-aware rename, direct form/new-record actions, and accurate first-run guidance.
4. Validate daily use: real sqld and concurrent sessions, complete reporting, printer configuration, telemetry setup, and measured interaction latency.

**Acceptance scenario for the product promise**

Give a person unfamiliar with the code only the first-run screen and in-app Help. They should be able to create a persistent customer/order application, add and edit records, select a customer beyond the first 200, paint a form, create and revise a query and report, connect them to a menu, close the program, and find everything again. During the exercise, have them open Help midway through an unsaved design, preview before saving, correct invalid input, and resize the terminal. No step should lose work or require a memorized asset name or SQL repair.

Then repeat daily operations against real sqld with two sessions, a long connection string, more than 10,000 records, an import failure, and a disconnected server. Validate database state and generated artifacts, not only on-screen success messages. Finally, verify that a kiosk session cannot change the database and a copied application cannot access host files through its scripts. Passing these checks would support the claim that users can fully use what Phosphor offers.

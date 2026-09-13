**GitHub follow-through for the 13 September 2026 review**

Tracking issue: [47](https://github.com/awksedgreep/phosphor/issues/47). The [original report](2026-09-13-code-and-ux.md) records behavior before fixes. Issue state and linked pull requests track implementation progress.

| Finding | GitHub issue |
|---|---|
| 1 | [#26 — P1: Lua's advertised sandbox permits host filesystem access](https://github.com/awksedgreep/phosphor/issues/26) |
| 2 | [#27 — P1: Read-only mode is enforced by command shape, not by the database](https://github.com/awksedgreep/phosphor/issues/27) |
| 3 | [#28 — P1: Table rebuilds discard constraints and triggers](https://github.com/awksedgreep/phosphor/issues/28) |
| 4 | [#29 — P1: An unchanged text default can trigger a rebuild and change future values](https://github.com/awksedgreep/phosphor/issues/29) |
| 5 | [#30 — P1: A user-defined `rowid` column can cause one edit to modify multiple records](https://github.com/awksedgreep/phosphor/issues/30) |
| 6 | [#31 — P1: Saving a non-tail insert switches the form to an unrelated record](https://github.com/awksedgreep/phosphor/issues/31) |
| 7 | [#32 — P1: Generated columns shift values onto the wrong field names](https://github.com/awksedgreep/phosphor/issues/32) |
| 8 | [#33 — P1: Enter in the focused detail pane opens the parent editor](https://github.com/awksedgreep/phosphor/issues/33) |
| 9 | [#34 — P1: Export and label output silently stop at 10,000 rows](https://github.com/awksedgreep/phosphor/issues/34) |
| 10 | [#35 — P1: Remote transactions do not provide the rollback behavior callers expect](https://github.com/awksedgreep/phosphor/issues/35) |
| 11 | [#36 — P1: Remote SQL splitting breaks ordinary text and stored scripts](https://github.com/awksedgreep/phosphor/issues/36) |
| 12 | [#37 — P1: A CSV parse error leaves an embedded transaction open with partial data](https://github.com/awksedgreep/phosphor/issues/37) |
| 13 | [#38 — P1: Opening Help destroys the screen the user needs help with](https://github.com/awksedgreep/phosphor/issues/38) |
| 14 | [#39 — P2: Running a builder destroys the design before the user can revise or save it](https://github.com/awksedgreep/phosphor/issues/39) |
| 15 | [#40 — P2: Database paths can hide every error and confirmation](https://github.com/awksedgreep/phosphor/issues/40) |
| 16 | [#41 — P2: Lists and forms allow the selection to move offscreen without scrolling](https://github.com/awksedgreep/phosphor/issues/41) |
| 17 | [#42 — P2: Foreign-key selection ends at the first 200 parent records](https://github.com/awksedgreep/phosphor/issues/42) |
| 18 | [#43 — P2: Generic LIMIT rewriting invalidates supported SQL](https://github.com/awksedgreep/phosphor/issues/43) |
| 19 | [#44 — P2: The first-run path and CRM guide do not get a new user to their first record reliably](https://github.com/awksedgreep/phosphor/issues/44) |
| 20 | [#45 — P2: Saved assets are difficult to discover and wire into an application](https://github.com/awksedgreep/phosphor/issues/45) |
| 21 | [#46 — P2: The main loop adds roughly a quarter-second to fast query results](https://github.com/awksedgreep/phosphor/issues/46) |

First implementation batch: #26, #29, and #38.

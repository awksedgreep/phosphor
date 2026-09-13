**First implementation batch from the code and UX review**

The [tracking issue](https://github.com/awksedgreep/phosphor/issues/47) lists all 21 findings. This batch addresses three of them:

- [#26](https://github.com/awksedgreep/phosphor/issues/26): both Lua runners now use an explicit library allowlist. File/process libraries, module loaders, base script loaders, and coroutines are unavailable. The database/UI helpers and calculation/formatting libraries remain available. The existing heap and instruction limits remain in place.
- [#29](https://github.com/awksedgreep/phosphor/issues/29): introspected defaults retain their SQL representation. Opening a default and accepting it unchanged produces no schema changes; a necessary rebuild preserves literal and expression defaults for future inserts. Editing a default still supports the designer's existing input rules.
- [#38](https://github.com/awksedgreep/phosphor/issues/38): Help suspends the complete previous screen, including unfinished input. Esc, F1, and q return to it. Pending database responses update the suspended screen, and mouse input while Help is open no longer reaches the underlying grid.

The new regressions reproduced the original failures before their fixes. They cover blocked Lua file access in addition to retained helpers, default round trips and real inserts after a rebuild, and Help during typing, memo editing, mouse input, and asynchronous EDIT/health completion. The generated manual explains the available Lua capabilities and Help's return behavior.

**Validation**

- `cargo test --all-features`: 158 suite entries passed, including nine new regression tests. Three existing optional integration tests return early because real sqld and the dbhealth extension are unavailable here.
- `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings`, and `git diff --check`: passed.
- `cargo test --release perf_budget -- --nocapture`: both checks passed; approximately 17.0 µs per embedded page and 5.9 µs per worker round trip in this run.
- `cargo build --release`: passed. The generated-manual synchronization test passed in the full suite.
- All nine selected terminal reels passed: `memo`, `scripting`, `create`, `tableeditor`, `qbe`, `forms`, `reports`, `helpdraft`, and `defaults`. These use the release binary and the existing PTY harness, with additional Help round trips in memo/scripting and two new regression reels. This run used a short database path at 100×30; it does not claim to fix the separately reported long-path or small-terminal layout issues.

To run the affected terminal flows without overwriting the repository's demo recordings:

```sh
cargo build --release
python3 -u - <<'PY'
import os, sys, tempfile
sys.path.insert(0, 'tools/demo')
import uitest as ui
root = tempfile.mkdtemp(prefix='pf-', dir='/tmp')
ui.DB = os.path.join(root, 'ui.db')
ui.OUT = os.path.join(root, 'casts')
ui.WORK = os.path.join(root, 'work')
ui.ENV = {'PHOSPHOR_EXT': ''}
selected = {'memo', 'scripting', 'create', 'tableeditor', 'qbe',
            'forms', 'reports', 'helpdraft', 'defaults'}
failed = False
for reel in ui.reels():
    if reel.name not in selected:
        continue
    ui.seed()
    errors = reel.run()
    print(reel.name, 'FAIL' if errors else 'PASS', errors)
    failed |= bool(errors)
print('Recordings:', root)
sys.exit(failed)
PY
```

**Remaining scope**

The original review and its failure logs describe the reviewed revision, before these fixes. The other 18 findings remain open. This batch restricts Lua's direct host capabilities; it retains the documented SQL API and does not establish a new database authorization boundary. Backend read-only enforcement and remote transaction correctness remain separate work.

Ordinary Esc and Ctrl-Q paths were also inspected for #38. Outside Help, they can still discard unsaved designs or text; script/memo Esc explicitly means cancel. A consistent save/discard flow across designers remains follow-up work in #47. This batch makes consulting Help lossless without changing those exit semantics.

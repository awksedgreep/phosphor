"""Run the remote regressions in a disposable source/database copy.

From the repository root:
    python3 docs/reviews/2026-09-13-data-fixes/run_remote_probes.py

The local protocol fixture is not a real sqld integration test. It also
inserts a competing record between the saved-record lookup and its next
page fetch, checking that a shifted position never changes the open row.
"""
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import time

repo = Path(__file__).resolve().parents[3]
root = Path(tempfile.mkdtemp(prefix="phosphor-data-probes-"))
for name in ["Cargo.toml", "Cargo.lock"]:
    shutil.copy2(repo / name, root / name)
shutil.copytree(repo / "src", root / "src")
(root / "docs").mkdir()
shutil.copy2(repo / "docs/MANUAL.md", root / "docs/MANUAL.md")
shutil.copy2(Path(__file__).with_name("remote_probes.rs"), root / "src/data_probes.rs")
with (root / "src/main.rs").open("a") as f:
    f.write("\n#[cfg(test)] mod data_probes;\n")

fixture = (repo / "docs/reviews/2026-09-13-evidence/hrana_fixture.py").read_text()
fixture = fixture.replace(
    "pathlib.Path(pathlib.Path('/tmp/phosphor-review-location').read_text())",
    "pathlib.Path(" + repr(str(root)) + ")",
).replace("'/tmp/phosphor-review-hrana-url'", repr(str(root / "url")))
# This occurs after SELECT's result has been snapshotted, before the
# next HTTP request. The surrounding table already contains IDs 1/50/100.
marker = "    rows=[[encode(v) for v in r] for r in cur.fetchall()]\n"
assert marker in fixture
fixture = fixture.replace(marker, marker + '''    if stmt['sql'].startswith('SELECT *, (SELECT count(*) FROM "remote_generated"'):
     if not conn.execute('SELECT 1 FROM remote_generated WHERE id=25').fetchone():
      conn.execute("INSERT INTO remote_generated(id,name) VALUES(25,'Concurrent')")
''', 1)
(root / "fixture.py").write_text(fixture)
proc = subprocess.Popen(["python3", "-u", str(root / "fixture.py")])
try:
    for _ in range(50):
        if (root / "url").exists():
            break
        time.sleep(0.05)
    env = dict(os.environ)
    env.pop("PHOSPHOR_EXT", None)
    env.pop("PHOSPHOR_TOKEN", None)
    env["PHOSPHOR_TEST_URL"] = (root / "url").read_text()
    env["CARGO_TARGET_DIR"] = str(repo / "target")
    result = subprocess.run(
        ["cargo", "test", "--manifest-path", str(root / "Cargo.toml"),
         "data_probes::", "--", "--nocapture", "--test-threads=1"],
        env=env, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True,
    )
    (root / "results.log").write_text(result.stdout)
    print(result.stdout)
    print("Artifacts:", root)
    raise SystemExit(result.returncode)
finally:
    proc.terminate()
    proc.wait(timeout=5)

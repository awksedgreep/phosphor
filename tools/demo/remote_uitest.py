#!/usr/bin/env python3
"""Run the terminal regressions against a disposable, real sqld server.

python3 tools/demo/remote_uitest.py --bin /path/to/sqld
Requires target/release/phosphor. Leaves recordings/results in /tmp/pfr-*.
"""
import argparse
import json
from pathlib import Path
import socket
import sqlite3
import subprocess
import tempfile
import time
import urllib.error
import urllib.request

import uitest as ui


def pipeline(url, requests):
    req = urllib.request.Request(url + "/v3/pipeline", data=json.dumps({"requests": requests}).encode(),
                                 headers={"Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=5) as response:
        body = json.load(response)
    for result in body["results"]:
        if result["type"] != "ok":
            raise RuntimeError(result)
    return body["results"]


def query(url, sql):
    return pipeline(url, [{"type": "execute", "stmt": {"sql": sql}}, {"type": "close"}])[0]["response"]["result"]["rows"]


def snapshot(url):
    schema = query(url, "SELECT type,name,sql FROM sqlite_schema ORDER BY name")
    tables = query(url, "SELECT name FROM sqlite_schema WHERE type='table' ORDER BY name")
    return schema, [(row[0]["value"], query(url, 'SELECT * FROM "' + row[0]["value"].replace('"', '""') + '"')) for row in tables]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--bin", required=True)
    parser.add_argument("--reels", nargs="*", default=["data", "scripting", "forms", "apps", "tableeditor", "kiosk",
                                                     "csvrollback", "sqltext", "rowidentity", "insertidentity", "generated",
                                                     "relations", "reports", "detailcrud", "completeoutput", "builders", "status80",
                                                     "scrolling", "lookup", "querysyntax", "tutorial"])
    args = parser.parse_args()
    root = Path(tempfile.mkdtemp(prefix="pfr-", dir="/tmp"))
    ui.DB, ui.OUT, ui.WORK = [str(root / n) for n in ("ui.db", "casts", "work")]
    ui.ENV = {"PHOSPHOR_EXT": "", "PHOSPHOR_TOKEN": ""}
    results = {}
    print("Artifacts:", root, flush=True)
    for reel in ui.reels():
        if reel.name not in args.reels:
            continue
        ui.seed()
        with sqlite3.connect(ui.DB) as conn:
            seed = "" if reel.fresh else "\n".join(conn.iterdump())
        directory = root / reel.name
        directory.mkdir()
        with socket.socket() as sock:
            sock.bind(("127.0.0.1", 0))
            address = f"127.0.0.1:{sock.getsockname()[1]}"
        url = "http://" + address
        with (directory / "server.log").open("w") as log:
            child = subprocess.Popen([str(Path(args.bin).resolve()), "--db-path", "test.sqld", "--http-listen-addr", address],
                                     cwd=directory, stdout=log, stderr=log)
        try:
            for _ in range(50):
                if child.poll() is not None:
                    raise RuntimeError((directory / "server.log").read_text())
                try:
                    query(url, "SELECT 1")
                    break
                except urllib.error.URLError:
                    time.sleep(0.1)
            else:
                raise RuntimeError("sqld did not start")
            if seed:
                pipeline(url, [{"type": "sequence", "sql": seed}, {"type": "close"}])
            before = snapshot(url) if reel.name == "kiosk" else None
            reel.argv = [url if arg == ui.DB else arg for arg in reel.argv]
            reel.restarts = [[url if arg == ui.DB else arg for arg in argv] if argv else None for argv in reel.restarts]
            errors = reel.run()
            if before is not None and before != snapshot(url):
                errors.append("read-only kiosk changed remote data/schema")
            results[reel.name] = errors
            print(reel.name, "FAIL" if errors else "PASS", errors, flush=True)
        finally:
            child.terminate()
            try:
                child.wait(timeout=5)
            except subprocess.TimeoutExpired:
                child.kill()
                child.wait()
    (root / "results.json").write_text(json.dumps(results, indent=2))
    if set(args.reels) != set(results):
        raise RuntimeError("unknown or missing reel")
    raise SystemExit(any(results.values()))


if __name__ == "__main__":
    main()

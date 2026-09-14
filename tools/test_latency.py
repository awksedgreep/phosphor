#!/usr/bin/env python3
"""Measure Enter-to-visible-result latency in a real terminal.

Build release first. --measure-only records a baseline without enforcing budgets.
No extension, optional Python package, or prepared database is required.
"""
import argparse
import codecs
import fcntl
import json
import os
from pathlib import Path
import pty
import select
import signal
import statistics
import struct
import sys
import tempfile
import termios
import time

sys.path.insert(0, str(Path(__file__).parent / "demo"))
from uitest import Screen


class Terminal:
    def __init__(self, binary, database, directory):
        self.screen = Screen(80, 24)
        self.decoder = codecs.getincrementaldecoder("utf-8")("replace")
        self.events = []
        self.start = time.monotonic()
        self.directory = Path(directory)
        self.pid, self.fd = pty.fork()
        if self.pid == 0:
            os.chdir(directory)
            os.execve(binary, [binary, database], {**os.environ, "TERM": "xterm-256color",
                                                  "PHOSPHOR_EXT": "", "PHOSPHOR_TOKEN": ""})
        fcntl.ioctl(self.fd, termios.TIOCSWINSZ, struct.pack("HHHH", 24, 80, 0, 0))

    def send(self, text):
        os.write(self.fd, text.encode())

    def respond(self, keys, predicate, timeout=5):
        start = time.monotonic()
        self.send(keys)
        self.wait(predicate, timeout)
        return (time.monotonic() - start) * 1000

    def resize(self, cols, rows):
        self.screen = Screen(cols, rows)
        self.events.append([time.monotonic() - self.start, "r", f"{cols}x{rows}"])
        fcntl.ioctl(self.fd, termios.TIOCSWINSZ, struct.pack("HHHH", rows, cols, 0, 0))

    def wait(self, predicate, timeout=5):
        deadline = time.monotonic() + timeout
        while not predicate(self.screen.text()):
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise AssertionError("Terminal timeout:\n" + self.screen.text())
            if select.select([self.fd], [], [], min(remaining, 0.01))[0]:
                data = os.read(self.fd, 65536)
                if not data:
                    raise AssertionError("Terminal exited before expected output")
                text = self.decoder.decode(data)
                self.events.append([time.monotonic() - self.start, "o", text])
                self.screen.feed(text)

    def query(self, sql, marker):
        self.send("." + sql)
        self.wait(lambda text: sql[-25:] in text.splitlines()[-2])
        start = time.monotonic()
        self.send("\r")
        self.wait(lambda text: marker in "\n".join(text.splitlines()[:4]))
        return (time.monotonic() - start) * 1000

    def close(self):
        start = time.monotonic()
        self.send("\x11")
        deadline = time.monotonic() + 1
        while time.monotonic() < deadline:
            if os.waitpid(self.pid, os.WNOHANG)[0]:
                break
            if select.select([self.fd], [], [], 0)[0]:
                try:
                    data = os.read(self.fd, 65536)
                except OSError:
                    data = b""
                if data:
                    self.events.append([time.monotonic() - self.start, "o", self.decoder.decode(data)])
            time.sleep(0.01)
        else:
            os.kill(self.pid, signal.SIGKILL)
        os.close(self.fd)
        header = {"version": 2, "width": 80, "height": 24, "timestamp": 0}
        with (self.directory / "latency.cast").open("w") as out:
            for entry in [header] + self.events:
                out.write(json.dumps(entry) + "\n")
        return (time.monotonic() - start) * 1000


def measure(binary, directory, database=":memory:"):
    terminal = Terminal(binary, database, directory)
    try:
        terminal.wait(lambda text: "Create your first table" in text)
        samples = [terminal.query(f"SELECT 42 AS probe_{i}", f"probe_{i}") for i in range(10)]
        terminal.send(".SELECT missing_latency_column\r")
        terminal.wait(lambda text: "no such column" in text)
        assert "probe_9" in terminal.screen.text(), "failed query replaced the preceding result"
        terminal.send("VALUES ('recovery_complete')\r")
        terminal.wait(lambda text: "recovery_complete" in "\n".join(text.splitlines()[:4]))
        # Enough computation to finish after the input handler returns,
        # exposing a stale terminal wait reliably even on a fast machine.
        computed = [terminal.query(f"WITH RECURSIVE s(n) AS (VALUES(1) UNION ALL SELECT n+1 FROM s WHERE n<20000) SELECT sum(n) AS compute_{i} FROM s", f"compute_{i}") for i in range(5)]
        return {"samples_ms": samples, "median_ms": statistics.median(samples), "max_ms": max(samples),
                "computed_samples_ms": computed, "computed_max_ms": max(computed)}
    finally:
        terminal.close()


def measure_operations(binary, directory, database=":memory:", rows=1000000000, cancel_budget=200):
    directory.mkdir()
    terminal = Terminal(binary, database, directory)
    measurements = {}
    try:
        terminal.wait(lambda text: "Create your first table" in text)
        recursive = f"WITH RECURSIVE s(n) AS (VALUES(1) UNION ALL SELECT n+1 FROM s WHERE n<{rows})"
        terminal.send(f".CREATE VIEW latency_slow AS {recursive} SELECT sum(n) AS amount FROM s; CREATE VIEW latency_rows AS {recursive} SELECT n FROM s; CREATE VIEW latency_fail AS SELECT abs(-9223372036854775808) AS amount\r")
        terminal.wait(lambda text: "ok (batch)" in text)
        terminal.send("report latency_slow\r")
        terminal.wait(lambda text: "REPORT · latency_slow" in text)
        measurements["report_progress_ms"] = terminal.respond("\x1bOQ", lambda text: "Preparing report" in text)
        measurements["report_help_ms"] = terminal.respond("\x1bOP", lambda text: "HELP" in text)
        measurements["report_details_ms"] = terminal.respond("\x1b[24~", lambda text: "STATUS & CONNECTION" in text)
        terminal.send("\x1b[24~")
        terminal.wait(lambda text: "HELP" in text and "STATUS & CONNECTION" not in text)
        terminal.send("\x1b")
        terminal.wait(lambda text: "Preparing report" in text and "HELP" not in text)
        terminal.resize(40, 12)
        terminal.wait(lambda text: "rows read" in text)
        terminal.resize(80, 24)
        terminal.wait(lambda text: "rows read" in text)
        measurements["report_cancel_ms"] = terminal.respond("\x1b", lambda text: "operation cancelled" in text)
        assert "REPORT · latency_slow" in terminal.screen.text(), "cancel lost the report design"
        terminal.send("\x1b")
        terminal.wait(lambda text: "REPORT · latency_slow" not in text)
        measurements["labels_progress_ms"] = terminal.respond("labels latency_slow\r", lambda text: "Preparing labels" in text)
        measurements["labels_cancel_ms"] = terminal.respond("\x1b", lambda text: "operation cancelled" in text)
        destination = directory / "output.csv"
        destination.write_text("keep this completed export\n")
        terminal.send("export latency_rows output.csv\r")
        terminal.wait(lambda text: "Exporting CSV" in text and "rows read" in text)
        measurements["export_help_ms"] = terminal.respond("\x1bOP", lambda text: "HELP" in text)
        terminal.send("\x1b")
        terminal.wait(lambda text: "Exporting CSV" in text and "HELP" not in text)
        measurements["export_cancel_ms"] = terminal.respond("\x1b", lambda text: "operation cancelled" in text)
        assert destination.read_text() == "keep this completed export\n", "cancel replaced destination"
        assert not list(directory.glob(".phosphor-output-*.tmp")), "cancel leaked temporary output"
        terminal.send("export latency_fail output.csv\r")
        terminal.wait(lambda text: "export incomplete" in text)
        assert destination.read_text() == "keep this completed export\n", "failed export replaced destination"
        terminal.send("export SELECT 42 AS answer output.csv\r")
        terminal.wait(lambda text: "exported 1 row(s)" in text)
        assert destination.read_text() == "answer\n42\n", "retry did not publish complete output"
        terminal.send("report latency_fail\r")
        terminal.wait(lambda text: "REPORT · latency_fail" in text)
        terminal.send("\x1bOQ")
        terminal.wait(lambda text: "integer overflow" in text)
        assert "REPORT · latency_fail" in terminal.screen.text(), "failure lost the report design"
        terminal.send("\x1b")
        terminal.wait(lambda text: "REPORT · latency_fail" not in text)
        terminal.send("labels latency_slow\r")
        terminal.wait(lambda text: "Preparing labels" in text)
    finally:
        measurements["quit_during_work_ms"] = terminal.close()
    for name, elapsed in measurements.items():
        assert elapsed < (cancel_budget if "cancel" in name else 200), measurements
    return measurements


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", default="target/release/phosphor")
    parser.add_argument("--measure-only", action="store_true")
    args = parser.parse_args()
    root = Path(tempfile.mkdtemp(prefix="pf-latency-", dir="/tmp"))
    result = measure(str(Path(args.binary).resolve()), root)
    if not args.measure_only:
        result["operations"] = measure_operations(str(Path(args.binary).resolve()), root / "operations")
    (root / "results.json").write_text(json.dumps(result, indent=2))
    print(json.dumps({"artifacts": str(root), **result}, indent=2), flush=True)
    if not args.measure_only:
        assert result["median_ms"] < 75, "median Enter-to-visible-result latency exceeds 75 ms"
        assert result["max_ms"] < 200, "a query exceeded the 200 ms terminal latency budget"
        assert result["computed_max_ms"] < 200, "a completed calculation waited too long to appear"


if __name__ == "__main__":
    main()

#!/usr/bin/env python3
"""Check terminal responsiveness with a real sqld server behind a delayed proxy."""
import argparse
import http.server
import json
from pathlib import Path
import socket
import subprocess
import tempfile
import threading
import time
import urllib.error
import urllib.request

from test_latency import measure, measure_operations


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--sqld-bin", required=True)
    parser.add_argument("--binary", default="target/release/phosphor")
    args = parser.parse_args()
    root = Path(tempfile.mkdtemp(prefix="pf-latency-remote-", dir="/tmp"))
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        address = f"127.0.0.1:{sock.getsockname()[1]}"
    upstream = "http://" + address
    with (root / "sqld.log").open("w") as log:
        child = subprocess.Popen([str(Path(args.sqld_bin).resolve()), "--db-path", "latency.sqld", "--http-listen-addr", address], cwd=root, stdout=log, stderr=log)

    class Proxy(http.server.BaseHTTPRequestHandler):
        def log_message(self, *_):
            pass

        def do_POST(self):
            body = self.rfile.read(int(self.headers["Content-Length"]))
            # Metadata opens remain fast. Delay the complete output request,
            # and delay probe replies until after the input event is handled.
            if b"latency_" in body and b"LIMIT 0" not in body and self.path.endswith("/cursor"):
                time.sleep(0.6)
            elif b"probe_" in body or b"compute_" in body:
                time.sleep(0.025)
            req = urllib.request.Request(upstream + self.path, data=body, headers={"Content-Type": "application/json"})
            try:
                with urllib.request.urlopen(req, timeout=10) as response:
                    payload = response.read()
                    self.send_response(response.status)
                    self.send_header("Content-Type", response.headers.get("Content-Type", "application/json"))
                    self.send_header("Content-Length", str(len(payload)))
                    self.end_headers()
                    self.wfile.write(payload)
            except (BrokenPipeError, ConnectionResetError):
                pass # quitting/cancelling a request closes its HTTP connection

    proxy = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Proxy)
    proxy.daemon_threads = True
    thread = threading.Thread(target=proxy.serve_forever, daemon=True)
    thread.start()
    url = f"http://127.0.0.1:{proxy.server_port}"
    try:
        for _ in range(50):
            try:
                with socket.create_connection(tuple(address.rsplit(":", 1)), timeout=0.1):
                    break
            except OSError:
                time.sleep(0.1)
        else:
            raise RuntimeError("sqld did not start")
        binary = str(Path(args.binary).resolve())
        results = measure(binary, root, url)
        results["operations"] = measure_operations(binary, root / "operations", url, rows=10000, cancel_budget=2000)
        (root / "results.json").write_text(json.dumps(results, indent=2))
        print(json.dumps({"artifacts": str(root), **results}, indent=2), flush=True)
        assert results["max_ms"] < 200 and results["computed_max_ms"] < 200, results
    finally:
        child.terminate()
        child.wait(timeout=5)
        proxy.shutdown()
        proxy.server_close()


if __name__ == "__main__":
    main()

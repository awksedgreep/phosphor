#!/usr/bin/env python3
"""Run the real-sqld regressions using a supplied or verified release binary.

python3 tools/test_sqld.py              # download pinned official release
python3 tools/test_sqld.py --bin /path/to/sqld
"""
import argparse
import hashlib
import os
from pathlib import Path
import platform
import subprocess
import tarfile
import tempfile
import urllib.request

VERSION = "libsql-server-v0.24.32"
ROOT = Path(__file__).resolve().parents[1]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--bin", default=os.environ.get("PHOSPHOR_SQLD_BIN"))
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="phosphor-sqld-test-") as temporary:
        if args.bin:
            binary = Path(args.bin).resolve()
        else:
            arch = {"arm64": "aarch64", "aarch64": "aarch64", "x86_64": "x86_64"}.get(platform.machine())
            target = {"Darwin": "apple-darwin", "Linux": "unknown-linux-gnu"}.get(platform.system())
            if not arch or not target:
                parser.error("no release for this platform; supply --bin")
            name = f"libsql-server-{arch}-{target}.tar.xz"
            base = f"https://github.com/tursodatabase/libsql/releases/download/{VERSION}"
            archive = Path(temporary, name)
            with urllib.request.urlopen(f"{base}/{name}", timeout=60) as response:
                archive.write_bytes(response.read())
            with urllib.request.urlopen(f"{base}/{name}.sha256", timeout=60) as response:
                expected = response.read().decode().split()[0]
            actual = hashlib.sha256(archive.read_bytes()).hexdigest()
            if actual != expected:
                raise RuntimeError("sqld release checksum mismatch")
            with tarfile.open(archive) as bundle:
                bundle.extractall(temporary, filter="data")
            binary = next(Path(temporary).rglob("sqld"))
            print(f"Verified {VERSION}: {actual}", flush=True)
        subprocess.run([str(binary), "--version"], check=True)
        env = {**os.environ, "PHOSPHOR_SQLD_BIN": str(binary), "PHOSPHOR_EXT": "", "PHOSPHOR_TOKEN": ""}
        subprocess.run(["cargo", "test", "--all-features", "against_real_sqld", "--", "--nocapture"],
                       cwd=ROOT, env=env, check=True)


if __name__ == "__main__":
    main()

#!/usr/bin/env python3
"""Scripted pty recorder → asciinema cast v2.

No human, no timing luck: scenarios are (at_seconds, keys) lists, the
child runs in a real pty at a fixed size, and output is captured with
timestamps. Render the .cast with agg to get a GIF.
"""
import codecs
import fcntl
import json
import os
import pty
import select
import signal
import struct
import sys
import termios
import time


def record(cmd, steps, out_path, cols=100, rows=30, env=None, tail=1.5, title=""):
    child_env = dict(os.environ)
    child_env.update(env or {})
    child_env["TERM"] = "xterm-256color"

    pid, fd = pty.fork()
    if pid == 0:  # child
        os.execvpe(cmd[0], cmd, child_env)

    fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", rows, cols, 0, 0))

    start = time.monotonic()
    events = []
    # Incremental decoding: a UTF-8 sequence split across pty reads must
    # not become replacement characters (it did — box-drawing glyphs are
    # three bytes each and reads cut anywhere).
    decoder = codecs.getincrementaldecoder("utf-8")("replace")
    idx = 0
    end_at = steps[-1][0] + tail if steps else tail
    alive = True
    while True:
        now = time.monotonic() - start
        while idx < len(steps) and steps[idx][0] <= now:
            try:
                os.write(fd, steps[idx][1].encode())
            except OSError:
                pass
            idx += 1
        try:
            r, _, _ = select.select([fd], [], [], 0.02)
        except (OSError, ValueError):
            break
        if r:
            try:
                data = os.read(fd, 65536)
            except OSError:
                alive = False
                break
            if not data:
                alive = False
                break
            text = decoder.decode(data)
            if text:
                events.append([round(time.monotonic() - start, 4), "o", text])
        if idx >= len(steps) and now > end_at:
            break

    if alive:
        try:
            os.kill(pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
    try:
        os.close(fd)
    except OSError:
        pass
    try:
        os.waitpid(pid, 0)
    except ChildProcessError:
        pass

    with open(out_path, "w") as f:
        f.write(
            json.dumps(
                {
                    "version": 2,
                    "width": cols,
                    "height": rows,
                    "timestamp": 0,
                    "title": title,
                    "env": {"TERM": "xterm-256color", "SHELL": "/bin/bash"},
                }
            )
            + "\n"
        )
        for ev in events:
            f.write(json.dumps(ev) + "\n")
    print(f"wrote {out_path} ({len(events)} events, {events[-1][0] if events else 0:.1f}s)")


def typing(at, text, cps=28.0):
    """Human-feeling typing: one step per character."""
    out = []
    t = at
    for ch in text:
        out.append((round(t, 3), ch))
        t += 1.0 / cps
    return out, t


if __name__ == "__main__":
    print("import me from a scenario script", file=sys.stderr)
    sys.exit(1)

#!/usr/bin/env python3
"""The four demo GIFs, fully scripted. Run from the repo root:
    python3 tools/demo/scenarios.py
Produces docs/demo/*.cast; render with agg (see demo.sh).
"""
import os
import sys

sys.path.insert(0, os.path.dirname(__file__))
from record import record, typing

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
BIN = os.path.join(ROOT, "target/release/phosphor")
DB = "/tmp/phosphor-demo.db"
EXT = os.path.abspath(os.path.join(ROOT, "../timeless-libsql/target/release/libdbhealth_ext.so"))
ENV = {"PHOSPHOR_EXT": EXT}
OUT = os.path.join(ROOT, "docs/demo")

ESC, ENTER, CTRL_Q = "\x1b", "\r", "\x11"
F2, F10 = "\x1bOQ", "\x1b[21~"

def taps(at, keys, gap=0.28):
    return [(round(at + i * gap, 3), k) for i, k in enumerate(keys)], at + len(keys) * gap

def browse():
    # First-letter seek: o jumps to orders, c back to customers.
    steps, t = taps(0.9, ["o", "c"], gap=0.7)
    steps += [(t + 0.3, ENTER)]                        # BROWSE
    s2, t = taps(t + 1.0, ["j", "j", "l", "l"])       # wander
    steps += s2
    steps += [(t + 0.4, ENTER)]                        # painted CUSTOMER CARD
    t += 3.0
    steps += [(t, ESC), (t + 0.5, ".")]               # dot prompt
    typed, t = typing(t + 0.7, "select city, sum(balance) as total from customers group by city")
    steps += typed
    steps += [(t + 0.3, ENTER)]                        # results grid
    t += 2.6
    steps += [(t, ".")]
    typed, t = typing(t + 0.2, "set theme amber")
    steps += typed
    steps += [(t + 0.2, ENTER)]
    t += 1.8
    steps += [(t, ".")]
    typed, t = typing(t + 0.2, "set theme green")
    steps += typed
    steps += [(t + 0.2, ENTER), (t + 1.4, CTRL_Q)]
    record([BIN, DB], steps, f"{OUT}/browse.cast", env=ENV, title="phosphor · browse")

def builders():
    steps, t = taps(0.9, ["c"])                       # seek customers
    steps += [(t + 0.3, "Q")]                          # QBE
    s2, t = taps(t + 1.2, ["j", "j", "j"])            # → balance row
    steps += s2
    steps += [(t + 0.3, ENTER)]                        # filter editor
    typed, t = typing(t + 0.5, "> 100")
    steps += typed
    steps += [(t + 0.2, ENTER), (t + 0.6, "s"), (t + 0.9, "s")]  # sort ▼
    t += 1.3
    steps += [(t, F2)]                                 # run
    t += 2.4
    steps += [(t, ESC)]                                # grid → sidebar
    s2, t = taps(t + 0.5, ["o"])                      # seek orders
    steps += s2
    steps += [(t + 0.3, "R")]                          # report designer
    s2, t = taps(t + 1.2, ["j", "j"])                 # → group by
    steps += s2
    s2, t = taps(t + 0.3, [" "] * 6)                  # cycle to region
    steps += s2
    steps += [(t + 0.4, F2)]                           # preview
    t += 2.8
    s2, t = taps(t, ["j", "j", "j"])                  # scroll bands
    steps += s2
    steps += [(t + 0.5, "w")]                          # write file
    t += 1.2
    steps += [(t, ESC)]                                # close pager
    steps += [(t + 0.6, "F")]                          # form designer (orders)
    t += 1.8
    steps += [(t, F2)]                                 # THE PAINTER
    t += 2.0
    s2, t = taps(t, ["l", "l", "l", "j", "j"])        # walk canvas
    steps += s2
    steps += [(t + 0.3, " ")]                          # place field
    steps += [(t + 0.8, "t")]                          # title text
    typed, t = typing(t + 1.0, "ORDERS")
    steps += typed
    steps += [(t + 0.2, ENTER)]
    t += 1.6
    steps += [(t, ESC), (t + 0.5, ESC), (t + 1.0, CTRL_Q)]
    record([BIN, DB], steps, f"{OUT}/builders.cast", env=ENV, title="phosphor · qbe, reports, painter")

def health():
    # dbhealth(every=2) is auto-sampling; the console is live on top.
    steps = [(0.9, F10)]
    steps += [(11.5, "s"), (13.2, ESC), (13.8, CTRL_Q)]
    record([BIN, DB], steps, f"{OUT}/health.cast", env=ENV, title="phosphor · DBHEALTH live")

def appmode():
    steps = [(2.4, "b")]                               # hotkey: Balances report
    steps += [(5.2, ESC)]                              # ONE Esc: home to the menu
    steps += [(6.6, "c")]                              # hotkey: Customers
    s2, t = taps(8.8, ["j", "j", "j"])
    steps += s2
    steps += [(t + 0.5, ESC), (t + 1.0, ESC)]         # grid → top → menu
    steps += [(t + 2.4, CTRL_Q)]
    record([BIN, "--app", DB], steps, f"{OUT}/appmode.cast", env=ENV,
           title="phosphor · --app: the database IS the application")

if __name__ == "__main__":
    os.makedirs(OUT, exist_ok=True)
    browse(); builders(); health(); appmode()

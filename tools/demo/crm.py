#!/usr/bin/env python3
"""The CRM demo to end all demos: an empty database becomes a working
CRM entirely through the UI — tables, foreign keys, data, a painted
form, split views, saved queries, a banded report, schema evolution,
and an application menu — then boots as `phosphor --app`.

Structure: CHAPTERS, each recorded separately against the same db
(snapshotted after every chapter so any chapter can be re-recorded in
isolation), verified by replaying the cast through the terminal
emulator and asserting on-screen markers, then merged into one long
cast for agg.

    python3 tools/demo/crm.py                # record all + verify + merge
    python3 tools/demo/crm.py --from 6       # resume from chapter 6
    python3 tools/demo/crm.py --only 3       # one chapter (from snapshot)
    python3 tools/demo/crm.py --merge        # just re-merge + render
"""
import json
import os
import subprocess
import sys

sys.path.insert(0, os.path.dirname(__file__))
from record import record, typing
from uitest import Screen

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
BIN = os.path.join(ROOT, "target/release/phosphor")
DB = "/tmp/phosphor-crm.db"
SNAP = os.path.join(ROOT, "docs/demo/crm")
OUT = os.path.join(ROOT, "docs/demo")
COLS, ROWS = 140, 40
FONT = "CaskaydiaMono Nerd Font Mono"

ESC, ENTER, CTRL_Q, TAB = "\x1b", "\r", "\x11", "\t"
PGUP, PGDN = "\x1b[5~", "\x1b[6~"
F1, F2, F3, F4, F5 = "\x1bOP", "\x1bOQ", "\x1bOR", "\x1bOS", "\x1b[15~"
F6, F7, F8, F9, F10 = "\x1b[17~", "\x1b[18~", "\x1b[19~", "\x1b[20~", "\x1b[21~"
UP, DOWN, LEFT, RIGHT = "\x1b[A", "\x1b[B", "\x1b[D", "\x1b[C"

# ── step builder helpers ─────────────────────────────────────────────


class Steps:
    """Accumulates (at, keys) with a moving clock."""

    def __init__(self, t0=1.2):
        self.t = t0
        self.steps = []

    def k(self, key, wait=0.45):
        self.steps.append((round(self.t, 3), key))
        self.t += wait
        return self

    def keys(self, keys, gap=0.3, wait=None):
        for key in keys:
            self.k(key, wait or gap)
        self.t -= (wait or gap) - gap
        return self

    def type(self, text, cps=28.0, wait=0.5):
        typed, self.t = typing(self.t, text, cps=cps)
        self.steps.extend(typed)
        self.t += wait
        return self

    def pause(self, secs):
        self.t += secs
        return self

    def click(self, col1, row1):
        """A real SGR mouse click (1-based col/row, as a terminal sends)."""
        press = f"\x1b[<0;{col1};{row1}M"
        release = f"\x1b[<0;{col1};{row1}m"
        self.steps.append((round(self.t, 3), press))
        self.steps.append((round(self.t + 0.15, 3), release))
        self.t += 1.0
        return self

    def done(self, extra=1.5):
        self.t += extra
        return self.steps, self.t


# ── per-chapter field-entry idioms ───────────────────────────────────



TABLES = ["contacts", "customers", "interactions", "orders"]  # alphabetical

def goto_table(s, name, browse=True):
    """Deterministic sidebar navigation. SidebarMove WRAPS, so absolute
    moves don't exist — but the unique-letter seek 'o' always lands on
    orders (only table starting with 'o'), giving every chapter a known
    anchor: from orders, j-steps reach the rest in table order."""
    assert name in TABLES
    s.k("o", 0.5)                                          # anchor: orders
    idx = TABLES.index(name)
    orders_idx = TABLES.index("orders")
    n = len(TABLES)
    for _ in range((idx - orders_idx) % n):
        s.k("j", 0.3)                                      # forward (wraps)
    if browse:
        s.k(ENTER, 1.2)

def new_record(s, fields, gap=0.55):
    """'a' then id-first entry: first Enter INSERTs, the rest UPDATE.
    Each field is typed then committed with Enter (Enter is the save key)."""
    s.k("a", 0.7)
    for f in fields:
        s.type(f, wait=gap)
        s.k(ENTER, gap)
    s.k(ESC, 0.7)


# ── the chapters ─────────────────────────────────────────────────────
# Each: (name, title, argv, tail, build(steps)->None, must, must_not)


def ch01_customers(s):
    """Genesis: C opens the designer; rename; two fields; F2; five
    customers typed in — inserts going live on every Enter."""
    s.pause(0.8).k("C", 1.0)
    s.k(UP, 0.4)
    s.type("customers", wait=0.6).k(ENTER, 0.5)          # rename table
    s.k(TAB, 0.4).k(F8, 0.6)                             # → id, insert field
    s.type("name", wait=0.45).k(ENTER, 0.45)             # TEXT by default
    s.k(F8, 0.6)
    s.type("city", wait=0.45).k(ENTER, 0.45)
    s.k(F2, 1.4)                                        # CREATE → BROWSE
    new_record(s, ["1", "Ada", "London"])
    new_record(s, ["2", "Grace", "Arlington"])
    new_record(s, ["3", "Edsger", "Austin"])
    new_record(s, ["4", "Barbara", "London"])
    new_record(s, ["5", "Donald", "Austin"])
    s.k(ENTER, 0.9)                                       # EDIT form on record 1
    s.k(ESC, 0.8)


def ch02_orders(s):
    """Orders with the first FOREIGN KEY — and the enforcement beat."""
    s.pause(0.8).k("C", 1.0)
    s.k(UP, 0.4)
    s.type("orders", wait=0.6).k(ENTER, 0.5)
    s.k(TAB, 0.4).k(F8, 0.6)
    s.type("customer_id", cps=32, wait=0.45).k(ENTER, 0.45)
    s.keys([F3, F3, F3, F3], gap=0.4)             # TEXT → INTEGER
    s.k(F10, 0.6)
    s.type("customers(id)", cps=32, wait=0.5).k(ENTER, 0.5)
    s.k(F8, 0.6)
    s.type("product", wait=0.4).k(ENTER, 0.45)
    s.k(F8, 0.6)
    s.type("qty", wait=0.4).k(ENTER, 0.45)
    s.keys([F3, F3, F3, F3], gap=0.35)            # INTEGER
    s.k(F8, 0.6)
    s.type("amount", wait=0.4).k(ENTER, 0.45)
    s.keys([F3], gap=0.35)                              # REAL
    s.k(F8, 0.6)
    s.type("region", wait=0.4).k(ENTER, 0.45)
    s.k(F2, 1.4)
    # Hand-enter one order — Enter is the save key all the way through.
    s.k("a", 1.0)
    s.k(ENTER, 0.9)                                       # insert (all NULL)
    s.type("1", wait=0.8).k(ENTER, 0.9)                   # customer 1
    s.type("compiler", wait=0.6).k(ENTER, 0.8)
    s.type("1", wait=0.6).k(ENTER, 0.7)
    s.type("99.0", wait=0.6).k(ENTER, 0.8)
    s.type("east", wait=0.6).k(ENTER, 0.8)
    s.k(ESC, 0.9)
    # The enforcement beat, loud and clear: an orphan order is refused.
    s.k(".", 0.7)
    s.type("insert into orders(customer_id, product) values (99, 'ghost')",
           cps=48, wait=0.7).k(ENTER, 1.1)
    # Backfill the legacy orders in one statement (paste-era data).
    s.type(
        "insert into orders(customer_id, product, qty, amount, region) values "
        "(2,'linker',2,45.0,'east'),(3,'semaphore',5,25.0,'west'),"
        "(4,'abstraction',3,120.0,'east'),(5,'tex',1,7.99,'west')",
        cps=48, wait=0.8,
    ).k(ENTER, 1.0)
    s.k(ESC, 0.6)


def ch03_evolution(s):
    """Designs change: ALTER adds columns, a wrong idea gets dropped."""
    s.pause(0.8).k(".", 0.6)
    s.type("alter table customers add column balance real default 0",
           cps=48, wait=0.7).k(ENTER, 0.9)
    s.type("alter table customers add column email text",
           cps=48, wait=0.7).k(ENTER, 0.9)
    s.type("alter table customers drop column email",
           cps=48, wait=0.7).k(ENTER, 0.9)
    s.k(ESC, 0.5)
    # A wrong idea, built properly so it can be dropped properly.
    s.k("C", 1.0)
    s.k(UP, 0.4)
    s.type("leads", wait=0.5).k(ENTER, 0.5)
    s.k(TAB, 0.4).k(F8, 0.6)
    s.type("source", wait=0.4).k(ENTER, 0.45)
    s.k(F2, 1.4)
    s.k(ESC, 0.6)                                          # browse leads -> sidebar
    s.k(".", 0.6)
    s.type("drop table leads", cps=40, wait=0.6).k(ENTER, 0.9)
    s.k(ESC, 0.5)                                          # prompt -> grid
    s.k(ESC, 0.5)                                          # grid -> sidebar
    s.k("r", 0.9)                                          # refresh (leads gone)
    s.k(".", 0.6)
    s.type("select * from leads", cps=40, wait=0.5).k(ENTER, 1.1)
    s.k(ESC, 0.5)                                          # prompt -> zombie grid
    s.k(ESC, 0.5)                                          # grid -> sidebar
    # Give Ada and Grace real balances through the EDIT form.
    goto_table(s, "customers")                    # browse customers
    s.k(ENTER, 0.9)                                       # EDIT Ada
    s.keys([DOWN, DOWN, DOWN], gap=0.35)                  # -> balance
    s.type("120.5", wait=0.55).k(ENTER, 0.7)
    s.k(ESC, 0.7)
    s.keys([DOWN], gap=0.5)                               # Grace (grid)
    s.k(ENTER, 0.9)
    s.keys([DOWN, DOWN, DOWN], gap=0.35)
    s.type("80", wait=0.55).k(ENTER, 0.7)
    s.k(ESC, 0.8)


def ch04_contacts(s):
    """Contacts (many per customer) + an interaction log, then the
    split view cycles through ALL of a customer's related tables."""
    s.pause(0.8).k("C", 1.0)
    s.k(UP, 0.4)
    s.type("contacts", wait=0.6).k(ENTER, 0.5)
    s.k(TAB, 0.4).k(F8, 0.6)
    s.type("customer_id", cps=32, wait=0.45).k(ENTER, 0.45)
    s.keys([F3, F3, F3, F3], gap=0.35)
    s.k(F10, 0.6)
    s.type("customers(id)", cps=32, wait=0.5).k(ENTER, 0.5)
    s.k(F8, 0.6)
    s.type("name", wait=0.4).k(ENTER, 0.45)
    s.k(F8, 0.6)
    s.type("role", wait=0.4).k(ENTER, 0.45)
    s.k(F8, 0.6)
    s.type("email", wait=0.4).k(ENTER, 0.45)
    s.k(F2, 1.4)
    new_record(s, ["1", "1", "Sue Black", "CTO", "sue@ada.io"])
    new_record(s, ["2", "1", "Ann Grey", "COO", "ann@ada.io"])
    new_record(s, ["3", "2", "Hal White", "Buyer", "hal@grace.io"])
    s.k(ESC, 0.6)
    # Interaction log, same pattern.
    s.k("C", 1.0)
    s.k(UP, 0.4)
    s.type("interactions", cps=32, wait=0.6).k(ENTER, 0.5)
    s.k(TAB, 0.4).k(F8, 0.6)
    s.type("customer_id", cps=32, wait=0.45).k(ENTER, 0.45)
    s.keys([F3, F3, F3, F3], gap=0.35)
    s.k(F10, 0.6)
    s.type("customers(id)", cps=32, wait=0.5).k(ENTER, 0.5)
    s.k(F8, 0.6)
    s.type("kind", wait=0.4).k(ENTER, 0.45)
    s.k(F8, 0.6)
    s.type("note", wait=0.4).k(ENTER, 0.45)
    s.k(F2, 1.4)
    new_record(s, ["1", "1", "call", "renewal discussed"])
    new_record(s, ["2", "1", "email", "quote sent"])
    new_record(s, ["3", "2", "visit", "walkthrough"])
    s.k(ESC, 0.7)                                          # form -> grid
    s.k(ESC, 0.6)                                          # grid -> sidebar
    # The payoff: customers ↔ {contacts, interactions, orders} on demand.
    goto_table(s, "customers")                    # seek customers, browse
    s.k("v", 1.1)                                          # contacts pane
    s.k("v", 1.0)                                          # cycle → interactions
    s.k("v", 1.0)                                          # cycle → orders
    s.k("v", 0.9)                                          # close


def ch05_painted_form(s):
    """CREATE SCREEN: fields placed by hand, a title, a box — and the
    painted card becomes the EDIT/insert surface."""
    goto_table(s, "customers", browse=False)  # select customers
    s.k("F", 1.0)                                          # form designer
    s.keys(["r"], gap=0.5)                                 # name required
    s.k(F2, 1.2)                                         # THE PAINTER
    s.keys(["l"] * 6, gap=0.22)                            # walk canvas
    s.k(TAB, 0.5)                                          # select name
    s.k(" ", 0.6)                                          # place it here
    s.keys(["j", "j"], gap=0.3)
    s.k(TAB, 0.5)
    s.k(" ", 0.6)
    s.keys(["j", "j"], gap=0.3)
    s.k(TAB, 0.5)
    s.k(" ", 0.6)
    s.keys(["+", "+"], gap=0.35)                           # widen balance
    s.k("t", 0.6)                                          # a title
    s.type("CUSTOMER CARD", cps=32, wait=0.4).k(ENTER, 0.6)
    s.k("b", 0.5)                                          # box corner 1
    s.keys(["j"] * 8 + ["h"] * 4, gap=0.18)
    s.k("b", 0.6)                                          # corner 2
    s.k(F6, 1.2)                                           # save layout
    s.k(ESC, 1.0)                                          # painter -> list
    s.k(ESC, 1.0)                                          # list -> sidebar
    goto_table(s, "customers")                             # browse customers
    s.k(ENTER, 1.4)                                        # painted EDIT card
    s.k(ESC, 1.0)
    s.k("a", 1.4)                                          # painted NEW record
    s.type("6", wait=0.8).k(ENTER, 1.2)                    # save blocks: name required
    s.k(DOWN, 0.7)                                         # -> name
    s.type("Niklaus", wait=0.8).k(ENTER, 1.2)              # INSERT succeeds now
    s.k(DOWN, 0.7)                                         # -> city
    s.type("Zurich", wait=0.8).k(ENTER, 1.2)               # UPDATE
    s.k(ESC, 1.1)


def ch06_split_mouse(s):
    """The split view as a way of life: chase with PgDn, wander the
    pane, and let a MOUSE CLICK re-point the whole screen."""
    goto_table(s, "customers")  # browse customers (cursor = Ada)
    s.k("v", 1.2)                                          # remembered: orders
    # One click on Grace's row re-points the whole screen.
    s.click(60, 4)
    s.keys([PGDN, PGDN], gap=0.9)                          # chase
    s.k(TAB, 0.7)                                          # into the pane
    s.keys(["j", "j", "k"], gap=0.35)
    s.k(TAB, 0.7)                                          # back to master
    s.k("v", 1.1)                                          # cycle -> contacts
    s.k("v", 1.1)                                          # cycle -> interactions
    s.k("v", 0.9)                                          # cycle -> close


def ch07_qbe(s):
    """Query by example: a filter, a sort, the generated SQL on screen,
    and the query saved under a name."""
    goto_table(s, "customers")  # browse customers
    s.k("Q", 1.0)
    s.keys(["j", "j", "j"], gap=0.35)                      # → balance row
    s.k(ENTER, 0.6)
    s.type("> 0", wait=0.4).k(ENTER, 0.6)                  # bare > means it
    s.keys(["s", "s"], gap=0.5)                            # sort ▼
    s.k(F2, 1.4)                                         # run
    s.k(ESC, 0.7)                                          # grid → designer? grid.
    # Save it (F6 on the designer; reopen if the grid took the keys).
    s.k("Q", 1.0)
    s.keys(["j", "j", "j"], gap=0.3)
    s.k(F6, 0.7)
    s.type("debtors", wait=0.4).k(ENTER, 0.8)
    s.k(F2, 1.2)                                         # run the saved spec
    s.k(ESC, 0.8)


def ch08_report(s):
    """The banded report: orders grouped by region, subtotals, grand
    total — written to a file and saved under a name."""
    s.pause(0.8).k(ESC, 0.5)                          # sidebar
    goto_table(s, "orders", browse=False)             # select orders
    s.k("R", 1.0)                                          # report designer
    s.keys(["j", "j"], gap=0.4)                            # → group by
    s.keys([" "] * 6, gap=0.4)                             # cycle → region
    s.k(F6, 0.9)                                           # save-as prompt (prefilled)
    s.k(ENTER, 0.7)                                        # accept the table name
    s.k(F2, 1.6)                                           # preview the bands
    s.keys(["j", "j", "j"], gap=0.4)                       # scroll the bands
    s.k("w", 1.0)                                          # write the file
    s.k(ESC, 0.8)                                          # close pager
    s.k(ESC, 0.6)                                          # designer closed
    goto_table(s, "customers", browse=False)               # select customers
    s.k("L", 1.2)                                          # mailing labels
    s.k(ESC, 0.8)


def ch09_prompt(s):
    """Saved queries run from the dot prompt; find + n; a theme beat."""
    goto_table(s, "customers")  # browse customers
    s.k(".", 0.6)
    s.type("run debtors", wait=0.4).k(ENTER, 1.2)
    s.k(".", 0.6)
    s.type("find Grace", wait=0.4).k(ENTER, 1.0)
    s.k("n", 0.7)                                          # again: nothing below
    s.k(".", 0.6)
    s.type("set theme amber", wait=0.4).k(ENTER, 1.0)
    s.pause(0.8)
    s.type("set theme green", wait=0.4).k(ENTER, 0.8)


def ch10_app_builder(s):
    """The Applications Generator: three menu items wired to the
    browse, the saved report, and the saved query."""
    s.pause(0.8).k("A", 1.2)
    s.k("r", 0.7)                                          # name the app (was "app")
    s.type("CRM", cps=32, wait=0.4).k(ENTER, 0.6)
    s.k("n", 0.7).k(ENTER, 0.5)                            # new item → label
    s.type("Customers", cps=32, wait=0.4).k(ENTER, 0.6)
    s.k("e", 0.6)                                          # action_ref
    s.type("customers", cps=32, wait=0.4).k(ENTER, 0.6)
    s.k("n", 0.7).k(ENTER, 0.5)
    s.type("Orders by region", cps=32, wait=0.4).k(ENTER, 0.6)
    s.k("e", 0.6)
    s.type("orders", cps=32, wait=0.4).k(ENTER, 0.6)       # reports are named after their table
    s.keys(["c", "c"], gap=0.5)                            # browse → query → report
    s.k("n", 0.7).k(ENTER, 0.5)
    s.type("Debtors", wait=0.4).k(ENTER, 0.6)
    s.k("e", 0.6)
    s.type("debtors", wait=0.4).k(ENTER, 0.6)
    s.keys(["c"], gap=0.5)                                 # → query
    s.k(F2, 1.4)                                         # live menu
    s.k("c", 1.2)                                          # hotkey: customers
    s.k(ESC, 0.8)                                          # home to the menu
    s.k(ESC, 0.8)                                          # menu → sidebar


def ch11_app_boots(s):
    """The reveal: `phosphor --app crm.db` — the database IS the
    application. This chapter boots straight into the menu."""
    s.pause(1.6)                                           # boot into the menu
    s.k("o", 1.8)                                          # orders by region
    s.keys(["j", "j"], gap=0.4)
    s.k(ESC, 1.1)                                          # home
    s.k("d", 1.6)                                          # debtors
    s.k(ESC, 1.1)                                          # home
    s.pause(1.0)                                           # closing shot: the menu


CHAPTERS = [
    ("01", "an empty db becomes customers", [], 2.0, ch01_customers,
     ["BROWSE customers", "Ada", "Donald", "inserted rowid"],
     ["error"]),
    ("02", "orders + the first foreign key", [], 2.0, ch02_orders,
     ["BROWSE orders", "FOREIGN KEY constraint failed", "inserted rowid",
      "compiler", "4 row(s) affected"],
     ["error: no such"]),
    ("03", "designs change: alter, drop, rethink", [], 2.0, ch03_evolution,
     ["balance", "drop table", "no such table: leads", "saved 1 field(s)"],
     ["error: no such table: customers"]),
    ("04", "contacts, interactions — and the split cycle", [], 2.0, ch04_contacts,
     ["BROWSE contacts", "BROWSE interactions", "contacts · customer_id",
      "interactions · customer_id", "orders · customer_id", "Sue Black"],
     ["error: no such"]),
    ("05", "CREATE SCREEN: the painted customer card", [], 2.0, ch05_painted_form,
     ["CUSTOMER CARD", "Niklaus", "inserted rowid"],
     ["error"]),
    ("06", "the split view, driven by keys and mouse", [], 2.0, ch06_split_mouse,
     ["orders · customer_id", "contacts · customer_id",
      "interactions · customer_id"],
     ["error"]),
    ("07", "query by example → saved query", [], 2.0, ch07_qbe,
     ["QUERY BY EXAMPLE", "> 0", "debtors"],
     ["error"]),
    ("08", "the banded report + mailing labels", [], 2.0, ch08_report,
     ["orders report", "region = east", "TOTAL", "wrote", "LABELS · customers"],
     ["error: no such"]),
    ("09", "the dot prompt runs the shop", [], 2.0, ch09_prompt,
     ["debtors", "found at row", "theme: amber"],
     ["error"]),
    ("10", "wiring the application menu", [], 2.0, ch10_app_builder,
     ["CRM", "Customers", "Orders by region", "Debtors"],
     ["error: no such"]),
    ("11", "--app: the database IS the application", ["--app"], 2.5, ch11_app_boots,
     ["CRM", "orders report", "TOTAL", "QUERY"],
     ["error: no such"]),
]


def record_chapter(idx, name, title, argv, tail, build, must, must_not, verify=True):
    path = os.path.join(SNAP, f"{name}.cast")
    s = Steps()
    build(s)
    steps, _ = s.done()
    env = {}
    argv_full = [BIN] + argv + [DB]
    record(argv_full, steps, path, env=env, title=f"phosphor · CRM · {title}",
           cols=COLS, rows=ROWS, tail=tail)
    ok = True
    if verify:
        seen = {m: False for m in must}
        polluted = {m: False for m in must_not}
        events = [json.loads(l) for l in open(path).read().splitlines()[1:] if l]
        screen = Screen(COLS, ROWS)
        for at, _, data in events:
            screen.feed(data)
            txt = screen.text()
            for m in must:
                if not seen[m] and m in txt:
                    seen[m] = True
            for m in must_not:
                if not polluted[m] and m in txt:
                    polluted[m] = True
        bad = [m for m in must if not seen[m]]
        pol = [m for m in must_not if polluted[m]]
        ok = not bad and not pol
        print(f"[{'PASS' if ok else 'FAIL'}] ch{name}: {title}")
        for m in bad:
            print(f"   missing: {m!r}")
        for m in pol:
            print(f"   UNEXPECTED: {m!r}")
    else:
        print(f"[REC] ch{name}: {title}")
    return ok


def snapshot(idx):
    subprocess.run(["cp", DB, os.path.join(SNAP, f"db-{idx}.sqlite")], check=True)


def restore(prev_idx):
    subprocess.run(
        ["cp", os.path.join(SNAP, f"db-{prev_idx}.sqlite"), DB], check=True
    )


def merge(out_path):
    header = json.dumps({"version": 2, "width": COLS, "height": ROWS})
    lines = [header]
    t0 = 0.0
    for idx, title, argv, tail, build, must, must_not in CHAPTERS:
        path = os.path.join(SNAP, f"{idx}.cast")
        raw = open(path).read().splitlines()
        events = [json.loads(l) for l in raw[1:] if l]
        dur = events[-1][0] if events else 0.0
        for at, kind, data in events:
            lines.append(json.dumps([round(t0 + at, 3), kind, data]))
        t0 += dur + 0.6  # a breath between chapters
    lines.append(json.dumps([round(t0, 3), "o", ""]))
    open(out_path, "w").write("\n".join(lines) + "\n")
    print(f"merged {len(CHAPTERS)} chapters -> {out_path} ({t0/60:.1f} min)")


def main():
    args = sys.argv[1:]
    do_merge = "--merge" in args
    only = None
    if "--only" in args:
        only = args[args.index("--only") + 1]
    start = 0
    if "--from" in args:
        start = int(args[args.index("--from") + 1])
    os.makedirs(SNAP, exist_ok=True)
    os.makedirs(OUT, exist_ok=True)

    if not do_merge:
        if not os.path.exists(DB):
            subprocess.run(["bash", os.path.join(os.path.dirname(__file__),
                                                 "crm-seed.sh"), DB], check=True)
        failures = []
        prev = None
        for i, (idx, title, argv, tail, build, must, must_not) in enumerate(CHAPTERS):
            if only and idx != only:
                prev = idx  # remember for snapshot restore
                continue
            if i < start:
                prev = idx
                continue
            if prev is not None:
                restore(prev)
            elif only and i > 0:
                # --only: restore the snapshot from the chapter before
                restore(CHAPTERS[i - 1][0])
            ok = record_chapter(idx, idx, title, argv, tail, build, must, must_not)
            if ok:
                snapshot(idx)
                prev = idx
            else:
                failures.append(idx)
                print(f"stopping at ch{idx} (fix, then --from {i})")
                break
        if failures:
            sys.exit(1)

    merge(os.path.join(OUT, "crm.cast"))


if __name__ == "__main__":
    main()

#!/usr/bin/env python3
"""Full-UI test sweep, GIF-pipeline style.

Scripted reels cover the application's screens and navigation paths. Each reel
records a real pty session AND carries ordered text assertions checked
against the captured output (ANSI-stripped) — so regressions fail
mechanically, and a green run leaves publishable GIFs behind.

    python3 tools/demo/uitest.py            # record + assert
    python3 tools/demo/uitest.py --render   # ...and render GIFs on green
"""
import json
import os
import re
import subprocess
import sys

sys.path.insert(0, os.path.dirname(__file__))
from record import record, typing

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
BIN = os.path.join(ROOT, "target/release/phosphor")
DB = "/tmp/phosphor-uitest.db"
EXT = os.path.abspath(
    os.path.join(ROOT, "../timeless-libsql/target/release/libdbhealth_ext.so")
)
ENV = {"PHOSPHOR_EXT": EXT}
OUT = os.path.join(ROOT, "docs/demo/ui")
WORK = "/tmp/phosphor-uitest-work"  # cwd for the app: report files land here

ESC, ENTER, CTRL_Q, TAB, SPACE = "\x1b", "\r", "\x11", "\t", " "
F1, F2, F3, F4, F5 = "\x1bOP", "\x1bOQ", "\x1bOR", "\x1bOS", "\x1b[15~"
F6, F7, F8, F9, F10 = "\x1b[17~", "\x1b[18~", "\x1b[19~", "\x1b[20~", "\x1b[21~"
F12 = "\x1b[24~"
UP, DOWN, LEFT, RIGHT = "\x1b[A", "\x1b[B", "\x1b[D", "\x1b[C"
PGDN, PGUP, HOME, END = "\x1b[6~", "\x1b[5~", "\x1b[H", "\x1b[F"

ANSI = re.compile(r"\x1b\[[0-9;?]*[a-zA-Z]|\x1b[()][0-9A-B]|\x1bO[A-Z]|\x1b[=>]")


class Screen:
    """Just enough terminal to reconstruct what is VISIBLE. ratatui
    paints with absolute cursor moves + SGR + text, and diff-renders
    only changed cells — so stream-grepping misses text that arrives
    one character at a time. Assertions must run against the screen."""

    TOKEN = re.compile(
        r"\x1b\[(?P<p>[0-9;?]*)(?P<c>[a-zA-Z])|\x1b[()][0-9A-B]|\x1bO[A-Z]|\x1b[=>]|\x1b[78]"
    )

    def __init__(self, cols, rows):
        self.cols, self.rows = cols, rows
        self.grid = [[" "] * cols for _ in range(rows)]
        self.r = self.c = 0
        # A pty read can split an escape sequence; hold the incomplete
        # tail until the next feed so it is parsed as one token (a split
        # `\x1b[38;2;…m` used to leak raw parameter bytes onto the grid).
        self.pending = ""

    def put(self, ch):
        if ch == "\r":
            self.c = 0
        elif ch == "\n":
            self.r = min(self.r + 1, self.rows - 1)
        elif ch == "\b":
            self.c = max(0, self.c - 1)
        elif ch >= " ":
            if self.r < self.rows and self.c < self.cols:
                self.grid[self.r][self.c] = ch
            self.c += 1

    def feed(self, data):
        data = self.pending + data
        self.pending = ""
        pos = 0
        for m in self.TOKEN.finditer(data):
            for ch in data[pos:m.start()]:
                self.put(ch)
            pos = m.end()
            c = m.groupdict().get("c")
            p = m.groupdict().get("p") or ""
            if c in ("H", "f"):
                parts = (p or "1;1").split(";")
                self.r = max(0, int(parts[0] or 1) - 1)
                self.c = max(0, int(parts[1] or 1) - 1) if len(parts) > 1 else 0
            elif c == "J":
                if p in ("2", "3"):
                    self.grid = [[" "] * self.cols for _ in range(self.rows)]
            elif c == "K":
                if self.r < self.rows:
                    for x in range(self.c, self.cols):
                        self.grid[self.r][x] = " "
            elif c == "A":
                self.r = max(0, self.r - int(p or 1))
            elif c == "B":
                self.r = min(self.rows - 1, self.r + int(p or 1))
            elif c == "C":
                self.c += int(p or 1)
            elif c == "D":
                self.c = max(0, self.c - int(p or 1))
        tail = data[pos:]
        # Hold an incomplete escape sequence for the next read.
        esc = tail.rfind("\x1b")
        if esc != -1:
            self.pending = tail[esc:]
            tail = tail[:esc]
        for ch in tail:
            self.put(ch)

    def text(self):
        return "\n".join("".join(row) for row in self.grid)

    def resize(self, cols, rows):
        self.grid = [(self.grid[y] if y < self.rows else [])[:cols] for y in range(rows)]
        self.grid = [row + [" "] * (cols - len(row)) for row in self.grid]
        self.cols, self.rows = cols, rows
        self.r, self.c = min(self.r, rows - 1), min(self.c, cols - 1)


class Reel:
    def __init__(self, name, title, argv=None, cols=100, rows=30):
        self.name = name
        self.title = title
        self.argv = argv or [BIN, DB]
        self.cols, self.rows = cols, rows
        self.restarts = 0
        self.steps = []
        self.expects = []
        self.t = 0.9

    def key(self, k, wait=0.35):
        self.steps.append((round(self.t, 3), k))
        self.t += wait
        return self

    def keys(self, ks, gap=0.28, wait=0.35):
        for k in ks:
            self.steps.append((round(self.t, 3), k))
            self.t += gap
        self.t += wait - gap
        return self

    def type(self, text, wait=0.35):
        typed, self.t = typing(self.t, text)
        self.steps.extend(typed)
        self.t += wait
        return self

    def pause(self, secs):
        self.t += secs
        return self

    def expect(self, marker):
        """The marker must be VISIBLE ON SCREEN at this point in the
        script (checked just before the next input fires)."""
        self.expects.append((round(self.t - 0.05, 3), marker, True))
        return self

    def expect_absent(self, marker):
        self.expects.append((round(self.t - 0.05, 3), marker, False))
        return self

    def restart(self):
        self.restarts += 1
        return self.key(CTRL_Q, wait=1.1)

    def resize(self, cols, rows):
        return self.key((cols, rows), wait=0.6)

    def run(self):
        cast = os.path.join(OUT, f"{self.name}.cast")
        self.key(CTRL_Q, wait=0.0)
        argv = self.argv
        if self.restarts:
            argv = [sys.executable, "-c",
                    "import subprocess, sys\nfor _ in range(int(sys.argv[1])):\n subprocess.run(sys.argv[2:], check=True)",
                    str(self.restarts + 1), *argv]
        record(argv, self.steps, cast, cols=self.cols, rows=self.rows, env=ENV, title=self.title, cwd=WORK)
        events = [
            json.loads(line)
            for line in open(cast).read().splitlines()[1:]
            if line
        ]
        screen = Screen(self.cols, self.rows)
        failures = []
        pending = sorted(self.expects)
        i = 0
        def check(t, marker, want):
            present = marker in screen.text()
            if present != want:
                verdict = "not on screen" if want else "unexpectedly on screen"
                failures.append(f"{verdict} at t={t}: {marker!r}")

        for at, kind, data in events:
            while i < len(pending) and pending[i][0] <= at:
                check(*pending[i])
                i += 1
            if kind == "r":
                screen.resize(*map(int, data.split("x")))
            else:
                screen.feed(data)
        for t, marker, want in pending[i:]:
            check(t, marker, want)
        return failures


def seed():
    subprocess.run(
        [os.path.join(ROOT, "tools/demo/seed.sh"), DB],
        check=True,
        capture_output=True,
    )
    os.makedirs(WORK, exist_ok=True)
    os.makedirs(OUT, exist_ok=True)
    # Fixture for the `data` reel's CSV import.
    with open(os.path.join(WORK, "people-import.csv"), "w") as f:
        f.write("id,name\n1,Imported Ada\n2,Imported Grace\n")
    with open(os.path.join(WORK, "malformed.csv"), "w") as f:
        f.write("name,city\nAda,London\nGrace\n")


def reels():
    out = []

    # ── A · navigation: seek, internals, browse motion, read-only ────
    r = Reel("nav", "navigation: seek · internals · browse · read-only")
    r.expect("internal (i)")
    r.key("o").expect("orders")                       # seek
    r.key(ENTER, 0.6).expect("BROWSE orders")
    r.key("G", 0.4).expect("row 8/8")                 # bottom
    r.key("g", 0.4).expect("row 1/8")
    r.key(END, 0.4).expect("region")                  # last column
    r.key(HOME, 0.4)
    r.key(ESC, 0.5)
    r.key("i", 0.5).expect("internal tables shown")
    r.keys(["d"] * 5, gap=0.3)                        # seek cycles the d's
    r.key(ENTER, 0.9).expect("(read-only)")           # dbhealth_now view
    r.key(ESC, 0.4)
    r.key("i", 0.5).expect("internal tables hidden")
    out.append(r)

    # ── B · CRUD: painted card, edit, required, insert, delete, find ─
    r = Reel("crud", "edit · insert · required · delete · find")
    r.key("c").key(ENTER, 0.6).expect("BROWSE customers")
    r.key(ENTER, 0.8).expect("CUSTOMER CARD")
    r.key(DOWN).key(ENTER, 0.4)                       # City shows 'London'
    r.type("Testville").key(ENTER, 0.6)               # typing REPLACES it
    r.expect("saved 1 field(s)").expect("Testville")
    r.key(ESC, 0.5)                                   # Enter saved already
    r.key("a", 0.6).expect("NEW customers record")
    r.key(F10, 0.6).expect('"Name" is required')
    r.key(ENTER, 0.3).type("Zed").key(ENTER, 0.6).expect("inserted rowid")
    r.key(ESC, 0.6)
    r.key("g", 0.3)                                   # find scans forward
    r.key(".", 0.3).type("find Zed").key(ENTER, 0.7).expect("found at row")
    r.key("x", 0.5).expect("press x again")
    r.key("x", 0.7).expect("row deleted")
    out.append(r)

    # ── B2 · the note editor: F3 opens a long field full-screen ─────
    r = Reel("memo", "the note editor: F3 edits a long TEXT field full-screen")
    r.key("n", 0.5).key(ENTER, 0.7).expect("BROWSE notes")
    r.key(ENTER, 0.7).expect("EDIT notes")
    r.key(TAB, 0.3).key(TAB, 0.3)                     # id -> customer_id -> note
    r.key(F3, 0.7).expect("NOTE ·")                   # the full-screen editor
    r.type(" - follow up")                            # caret opens at the end
    r.key(ENTER, 0.4)
    r.type("next week")
    r.key(F1, 0.5).expect("HELP")
    r.key(ESC, 0.5).expect("NOTE ·").expect("next week")
    r.key(F6, 0.6).expect("note saved to")            # fold back into the field
    r.key(F10, 0.8).expect("saved")                   # the form writes the row
    r.key(ENTER, 0.8).expect("EDIT notes")            # reopen: the newline shows
    r.key(TAB, 0.3).key(TAB, 0.3)
    r.expect("follow up␤next week")
    r.key(ESC, 0.4).key(ESC, 0.4)
    out.append(r)

    # ── C · the dot prompt: SQL, errors, completion, themes ──────────
    r = Reel("prompt", "the dot prompt: SQL · errors · completion · themes")
    r.key(".", 0.3).type("select count(*) as customers_n from customers")
    r.key(ENTER, 0.7).expect("customers_n").expect("1 row(s)")
    r.key(".", 0.3).type("selek 1").key(ENTER, 0.7).expect("syntax error")
    r.type("sel").key(TAB, 0.4)                       # completes 'select'
    r.type(" 6*7 as answer").key(ENTER, 0.7).expect("answer")
    r.key(".", 0.3).type("set theme amber").key(ENTER, 1.2).expect("theme: amber")
    r.key(".", 0.3).type("set theme paper").key(ENTER, 1.2).expect("theme: paper")
    r.key(".", 0.3).type("set theme blue").key(ENTER, 1.2).expect("theme: blue")
    r.key(".", 0.3).type("set theme green").key(ENTER, 0.8).expect("theme: green")
    out.append(r)

    # ── D · QBE: show/sort/filter, live SQL, run, save, replay ───────
    r = Reel("qbe", "query by example: filters · live SQL · save · run")
    r.key("c").key("Q", 0.7).expect("QUERY BY EXAMPLE")
    r.key(SPACE, 0.4)                                 # hide id
    r.keys([DOWN] * 3, gap=0.25)                      # → balance
    r.key(ENTER, 0.3).type("> 100").key(ENTER, 0.4)
    r.key("s", 0.3).key("s", 0.5)                     # sort ▼
    # The SQL wraps in the QBE panel: assert the pieces per line.
    r.expect('WHERE "balance" > 100').expect('ORDER BY "balance"')
    r.key(F2, 0.8).expect("5 row(s)")
    r.key("Q", 0.6).expect('WHERE "balance" > 100')  # return to the same design
    r.key(F6, 0.4).type("big-spenders").key(ENTER, 0.6)
    r.expect('saved query "big-spenders"')
    r.key(ESC, 0.4)
    r.key(".", 0.3).type("run big-spenders").key(ENTER, 0.7).expect("5 row(s)")
    out.append(r)

    # ── E · reports & labels: bands, totals, write, save ─────────────
    r = Reel("reports", "banded reports · labels")
    r.key("o").key("R", 0.7).expect("REPORT · orders")
    r.key(ENTER, 0.3).type(" by region").key(ENTER, 0.4)  # extend title
    r.keys([DOWN] * 2, gap=0.25)
    r.keys([SPACE] * 6, gap=0.3)                      # group: region
    r.key(F6, 0.6).type("orders-by-region").key(ENTER, 0.7)  # save as (renameable)
    r.expect("saved report")
    r.key(F2, 1.0).expect("region = east").expect("subtotal").expect("TOTAL (8 rows)")
    r.keys(["j"] * 3, gap=0.25)
    r.key("w", 0.6).expect("wrote report_orders-by-region.txt")
    r.key(ESC, 0.5).expect("REPORT · orders-by-region").key(ESC, 0.4)
    r.key("c").key("L", 0.8).expect("LABELS · customers").expect("Zurich")
    r.key(ESC, 0.4)
    out.append(r)

    # ── F · forms & painter: craft, paint, runtime render ────────────
    r = Reel("forms", "form designer · the painter · painted EDIT")
    r.key("o").key("F", 0.7).expect("FORM · orders")
    r.key(SPACE, 0.4)                                 # hide id
    r.key(DOWN).key("r", 0.4)                         # require customer
    r.key(ENTER, 0.3)
    r.type("Who").key(ENTER, 0.4)                     # typing replaces prefill
    r.key("n", 0.5)                                   # add a computed field
    r.type("qty * amount").key(ENTER, 0.4)            # its SQL expression
    r.key(ENTER, 0.3).type("Total").key(ENTER, 0.4).expect("Total")  # relabel
    r.key(F6, 0.6).expect("saved form")
    r.key(F2, 0.8).expect("FORM PAINTER · orders")
    r.key(TAB, 0.4)                                   # select `product`
    r.keys([DOWN] * 12, gap=0.12)                     # to the clear bottom row
    r.key(SPACE, 0.4)                                 # place it there
    r.keys([UP] * 12, gap=0.12)                       # back to the freed gap
    r.key("t", 0.3).type("ORDER ENTRY").key(ENTER, 0.5)
    r.key(LEFT, 0.2).key(UP, 0.2)                     # one cell of padding
    r.key("b", 0.3).keys([RIGHT] * 12 + [DOWN] * 2, gap=0.12).key("b", 0.5)
    r.key(F6, 0.6).expect("saved painted form")
    r.key(ESC, 0.4).key(ESC, 0.5)
    r.key(ENTER, 0.6)                                 # browse orders
    r.key(ENTER, 1.0).expect("ORDER ENTRY").expect("Who:").expect("Total")
    r.key(ESC, 0.4).key(ESC, 0.4)
    out.append(r)

    # ── G · applications generator: items CRUD, live menu, hotkey ────
    r = Reel("apps", "applications generator · live menu · hotkeys")
    r.key("A", 0.7).expect("APPLICATIONS GENERATOR · crm")
    r.key("n", 0.5).expect("New item")
    r.key(ENTER, 0.3)                                 # edits the NEW item
    r.type("Zap orders").key(ENTER, 0.4)              # typing replaces prefill
    r.key("e", 0.3).type("orders").key(ENTER, 0.5).expect("Zap orders")
    r.keys(["["] * 2, gap=0.3)                        # reorder up
    r.key(F2, 0.8).expect("CRM").expect("Zap orders").expect("Customers")
    r.key("z", 0.8).expect("BROWSE orders")           # hotkey runs it
    r.key(ESC, 0.4).key(ESC, 0.5)
    r.expect("APPLICATIONS GENERATOR · crm")         # cursor is still on Zap
    r.key("x", 0.6).expect_absent("Zap orders")       # gone
    r.key("r", 0.5).type("crm-app").key(ENTER, 0.6)   # rename the app
    r.expect("APPLICATIONS GENERATOR · crm-app")
    r.key(ESC, 0.4)
    out.append(r)

    # ── T · the TABLE DESIGNER: structure screen → real table ────────
    r = Reel("create", "the table designer: fields → CREATE TABLE → first record")
    r.key(".", 0.3).type("create gadgets").key(ENTER, 0.7)
    r.expect("TABLE DESIGNER · gadgets")
    r.expect('"id" INTEGER PRIMARY KEY')              # live SQL preview
    r.key(F8, 0.4)                                    # insert a field
    r.type("label").key(ENTER, 0.4)                   # just TYPE the name
    r.key(F5, 0.4).expect('"label" TEXT NOT NULL')    # F5 = required
    r.key(F8, 0.4).key(F3, 0.4)                       # new field → REAL
    r.key(F7, 0.3).type("1").key(ENTER, 0.5).expect('"field3" REAL DEFAULT 1')
    # [ moves the field up: the SQL preview now shows it right
    # after the pk (the DEFAULT tail wraps, so assert the head).
    r.key("[", 0.5).expect('KEY,  "field3"')
    r.key("]", 0.5).expect_absent('KEY,  "field3"')   # and back down
    r.key(F2, 0.8).expect("BROWSE gadgets").expect("created \"gadgets\"")
    r.key("a", 0.6).expect("NEW gadgets record")
    r.key("\t", 0.3)                                  # Tab to the next field
    r.type("widget").key(ENTER, 0.6)                  # the form is LIVE: type
    r.expect("inserted rowid 1")
    # Keep typing after the insert: the next Enter UPDATEs, and the
    # form must keep showing the saved value (not a stale NULL).
    r.type("2.5").key(ENTER, 0.6)
    r.expect("saved 1 field(s)").expect("2.5")
    r.key(ESC, 0.5)
    out.append(r)

    # ── P · record paging: hold the key, fly through the file ────────
    r = Reel("paging", "record paging: PgDn through 500 records")
    r.key("p").key(ENTER, 0.6).expect("BROWSE people")
    r.key(ENTER, 0.7).expect("EDIT people · 1/500")
    # Held paging ACCELERATES: streak k strides min(1 + k//6, 10), and
    # a >150ms pause resets. These positions mirror that formula.
    r.keys([PGDN] * 30, gap=0.05, wait=0.6).expect("EDIT people · 91/500")
    r.keys([PGDN] * 40, gap=0.04, wait=0.6).expect("EDIT people · 245/500")
    r.expect("record 245 of 500")                     # the DATA flips too
    r.keys([PGUP] * 3, gap=0.25, wait=0.5).expect("EDIT people · 242/500")
    # Dirty edit commits on page: type into note, page, check the grid.
    r.keys([DOWN] * 2, gap=0.2)
    r.key(ENTER, 0.3).type("edited in flight").key(ENTER, 0.5)
    r.expect("saved 1 field(s)")                      # Enter saved it
    r.key(PGDN, 0.6).expect("EDIT people · 243/500")
    r.key(ESC, 0.5)
    r.key("g", 0.3)                                   # find scans forward
    r.key(".", 0.3).type("find edited in flight").key(ENTER, 0.8)
    r.expect("found at")
    out.append(r)

    # ── H · health console + contextual help ─────────────────────────
    r = Reel("health", "DBHEALTH live · contextual help")
    r.key(F10, 1.2).expect("DBHEALTH · dbhealth").expect("LIVE")
    r.key("s", 1.0).expect("sampled")
    r.key(F1, 0.8).expect("THE DBHEALTH CONSOLE")
    r.key(RIGHT, 0.6).expect("CONNECTING")
    r.key(ESC, 0.4).key(ESC, 0.5)
    r.key(F1, 0.7).expect("BROWSING & EDITING")
    r.key(ESC, 0.4)
    out.append(r)

    # ── R · relations: declared FKs become child panes on the form ───
    r = Reel("relations", "foreign keys → SET RELATION: child panes on the form")
    r.key(".", 0.3).type("CREATE TABLE accounts(id INTEGER PRIMARY KEY, name TEXT)")
    r.key(ENTER, 0.5)
    r.key(".", 0.3).type("create invoices").key(ENTER, 0.7)
    r.expect("TABLE DESIGNER · invoices")
    r.key(F8, 0.3).type("item").key(ENTER, 0.4)
    r.key(F8, 0.3).type("account_id").key(ENTER, 0.4)
    r.keys([F3] * 4, gap=0.2, wait=0.4)               # TEXT → INTEGER
    r.key(F10, 0.3).type("accounts").key(ENTER, 0.5)  # F10 = foreign key
    r.expect('REFERENCES "accounts"')
    r.key(F2, 0.8).expect("BROWSE invoices")
    r.key(ESC, 0.5)
    r.key(".", 0.3).type("INSERT INTO accounts(name) VALUES ('Ada'),('Grace')")
    r.key(ENTER, 0.5)
    r.key(".", 0.3)
    r.type("INSERT INTO invoices(item, account_id) VALUES ('modem',1),('coax',1),('router',2)")
    r.key(ENTER, 0.6).key(ESC, 0.4).key(ESC, 0.5)    # prompt → grid → sidebar
    r.key("a", 0.4).key(ENTER, 0.7).expect("BROWSE accounts")
    r.key(ENTER, 0.8).expect("EDIT accounts · 1/2")
    r.expect("invoices (2)").expect("coax")           # Ada's pane, live
    r.key(PGDN, 0.8).expect("invoices (1)").expect("router")
    r.key(F4, 0.9).expect("router").expect_absent("modem")  # filtered browse
    r.key(ESC, 0.5)
    # Split BROWSE, and the H orientation toggle (issue #18).
    r.key(ENTER, 0.6).expect("BROWSE accounts")
    r.key("v", 0.9).expect("invoices · account_id")
    r.key("H", 0.7).expect("stacked")
    r.key("v", 0.7)                                   # single link: closes
    r.key(ESC, 0.4)
    out.append(r)

    # ── S · scripting: lifecycle events + menu action + the editor ───
    r = Reel("scripting", "Lua scripting: lifecycle · menu action · editor")
    # Bind an OnChange lifecycle script that uppercases the note.
    r.key(".", 0.3).type(
        'script people OnChange if field == "note" then set("note", string.upper(record.note)) end'
    )
    r.key(ENTER, 0.7)
    r.key(ESC, 0.4)                                   # leave the prompt
    r.key("p", 0.5).key(ENTER, 0.6).expect("BROWSE people")
    r.key(ENTER, 0.7).expect("EDIT people")
    r.keys([DOWN] * 3, gap=0.25)                      # -> note
    r.type("padded").key(ENTER, 0.8).expect("PADDED") # OnChange rewrote it
    r.key(ESC, 0.4).key(ESC, 0.5)
    # A script menu item, authored in the full-screen editor, run by hotkey.
    r.key("A", 0.7).expect("APPLICATIONS GENERATOR · crm")
    r.key("n", 0.5).key(ENTER, 0.4).type("Tally").key(ENTER, 0.4)
    r.keys(["c"] * 4, gap=0.4)                        # browse→query→report→sql→script
    r.key("E", 0.8).expect("SCRIPT · crm · Tally")
    r.type('local n = scalar("select count(*) from people")')
    r.key(ENTER, 0.5)
    r.type('say("people: " .. n)')
    r.key(F1, 0.5).expect("HELP")
    r.key(ESC, 0.5).expect("SCRIPT · crm · Tally").expect('say("people: " .. n)')
    r.key(F6, 0.9).expect("APPLICATIONS GENERATOR · crm")  # saved, back to designer
    r.key(F2, 0.9).expect("CRM").expect("Tally")
    r.key("t", 1.2).expect("people: 500")
    r.key(ESC, 0.5).key(ESC, 0.5)
    out.append(r)

    # ── P · PICTURE masks + the F7 foreign-key picker ────────────────
    r = Reel("pickers", "PICTURE masks · F7 foreign-key value picker")
    r.key("o", 0.5).key(ENTER, 0.6).expect("BROWSE orders")
    r.key("F", 0.7).expect("FORM · orders")
    r.keys([DOWN] * 2, gap=0.25)                      # id -> customer -> product
    r.key("m", 0.4).type("99-99").key(ENTER, 0.5).expect("99-99")
    r.key(F6, 0.7).expect("saved form")
    r.key(ESC, 0.5)                                   # back to the grid
    r.key(ENTER, 0.8).expect("EDIT orders")
    r.keys([DOWN] * 6, gap=0.2)                       # -> customer_id (FK)
    r.key(F7, 0.8).expect("PICK · customers")
    r.key(DOWN, 0.3).key(ENTER, 0.6)                  # choose Grace
    r.key(F10, 0.7)                                   # save + close
    r.key(ESC, 0.4)
    out.append(r)

    # ── J · QBE joins (J) + GROUP BY (g) ─────────────────────────────
    r = Reel("qbeextras", "QBE: FK joins (J) and GROUP BY (g)")
    r.key("o", 0.5).key(ENTER, 0.6).expect("BROWSE orders")
    r.key("Q", 0.7).expect("QUERY BY EXAMPLE · orders")
    r.key("J", 0.6).expect("JOIN").expect("customers")
    r.key("g", 0.6).expect("GROUP").expect("count(*) AS n")
    r.key(F2, 0.9).expect("row(s)")
    r.key(ESC, 0.4).key(ESC, 0.4)
    out.append(r)

    # ── D · data plumbing: CSV in/out + the advisor ──────────────────
    r = Reel("data", "CSV import/export · the index/vacuum advisor")
    r.key(".", 0.3)
    r.type("CREATE TABLE csvtest(id INTEGER PRIMARY KEY, name TEXT)")
    r.key(ENTER, 0.6)
    r.type(f"import csvtest {WORK}/people-import.csv").key(ENTER, 0.7)
    r.expect("imported 2 row(s)")
    r.type(f"export csvtest {WORK}/csvtest-out.csv").key(ENTER, 0.7)
    r.expect("exported 2 row(s)")
    r.type("advise").key(ENTER, 0.9)
    r.expect("INDEX & VACUUM ADVISOR").expect("orders.customer_id has no index")
    r.key(ESC, 0.5)
    out.append(r)

    # ── K · read-only kiosk: every write refused ─────────────────────
    r = Reel("kiosk", "--app --readonly: browsable, never writable",
             argv=[BIN, "--app", "--readonly", DB])
    r.pause(1.0).expect("Customers")
    r.key("c", 1.0).expect("BROWSE customers")
    r.key("a", 0.7).expect("read-only")
    r.key(".", 0.3).type("set theme amber").key(ENTER, 0.6).expect("session only")
    r.type("WITH x AS (SELECT 1) INSERT INTO customers(name) SELECT 'bad' FROM x RETURNING * -- limit").key(ENTER, 0.8).expect("read-only")
    r.type("SELECT count(*) AS unchanged_eight FROM customers").key(ENTER, 0.7).expect("unchanged_eight")
    r.key(ESC, 0.4).key(ESC, 0.5)
    out.append(r)

    # ── C · column widths (+/-) and frozen columns (f) ───────────────
    r = Reel("columns", "resize columns (+/-) and freeze them (f)")
    r.key("o", 0.5).key(ENTER, 0.6).expect("BROWSE orders")
    r.key("=", 0.5).expect("cells")                   # widen id
    r.key("l", 0.3).key("=", 0.4).expect("cells")     # widen customer
    r.key("f", 0.5).expect("frozen")                  # freeze through cursor
    r.key("l", 0.3).key("l", 0.3)                     # scroll right; frozen stays
    r.key("f", 0.5).expect("unfrozen")
    r.key(ESC, 0.4)
    out.append(r)

    # ── E2 · the TABLE EDITOR: alter columns, drop a table (confirm) ─
    r = Reel("tableeditor", "TABLE EDITOR (E): alter columns · drop a table")
    r.key("o", 0.5).key(ENTER, 0.6).expect("BROWSE orders")
    r.key("E", 0.8).expect("TABLE EDITOR · orders")
    r.key(F8, 0.4).type("note").key(ENTER, 0.4)       # add a column
    r.key(F2, 1.0).expect("applied 1 change")
    r.key("E", 0.8).expect("TABLE EDITOR · orders")
    r.key(F9, 0.5)                                    # cursor lands on the last field
    r.key(F2, 1.0).expect("applied 1 change")         # drop the column
    r.key("E", 0.6).key(F3, 0.4).expect("cannot safely rebuild")
    r.key(F2, 0.6).expect("TABLE EDITOR · orders").expect("cannot safely rebuild")
    r.key(ESC, 0.4)
    # A throwaway table, dropped with the two-press confirm.
    r.key(".", 0.3).type("CREATE TABLE scratch(x TEXT)").key(ENTER, 0.6)
    r.key(ESC, 0.4).key(ESC, 0.4)                     # prompt -> grid -> sidebar
    r.key("s", 0.5).key(ENTER, 0.6).expect("BROWSE scratch")
    r.key("E", 0.8).expect("TABLE EDITOR · scratch")
    r.key("D", 0.5).expect("DROP TABLE")
    r.key("D", 0.8).expect("dropped table")
    r.key(ESC, 0.4)
    out.append(r)

    # ── I · app mode: the database IS the application ────────────────
    r = Reel("appmode", "--app: menu · report · single-Esc home",
             argv=[BIN, "--app", DB])
    r.pause(1.2).expect("Customers")
    r.key("b", 1.2).expect("TOTAL (8 rows)")          # Balances report
    r.key(ESC, 0.8).expect("hotkey letters")          # ONE Esc → menu
    r.key("c", 0.8).expect("BROWSE customers")
    r.key(ESC, 0.4).key(ESC, 0.6)                     # top-level → menu
    out.append(r)

    # Help must resume a live draft, including an unfinished name or
    # filter. Saving/using it afterward proves the buffer survived.
    r = Reel("helpdraft", "Help preserves table and query drafts while typing")
    r.key(".").type("create help_draft").key(ENTER, 0.6)
    r.key(F8).type("lab")
    r.key(F1, 0.5).expect("HELP")
    r.key(ESC, 0.5).expect("TABLE DESIGNER · help_draft")
    r.type("el").key(ENTER).expect('"label" TEXT')
    r.key(F2, 0.7).expect("BROWSE help_draft")
    r.key("Q", 0.5).key(DOWN).key(ENTER).type("widget")
    r.key(F1, 0.5).expect("HELP")
    r.key(F1, 0.5).expect("QUERY BY EXAMPLE")
    r.key(ENTER).expect('WHERE "label" = \'widget\'')
    r.key(F6).type("help_lookup").key(F1, 0.5).expect("HELP")
    r.key(ESC, 0.5).key(ENTER, 0.6).expect('saved query "help_lookup"')
    r.key(ESC)
    out.append(r)

    # Opening F7 and accepting an untouched SQL literal must be a
    # no-op. A subsequent insert must still receive the original text.
    r = Reel("defaults", "TABLE EDITOR preserves an unchanged text default")
    r.key(".").type("CREATE TABLE defaults(id INTEGER PRIMARY KEY, state TEXT DEFAULT 'new')")
    r.key(ENTER, 0.6).key(ESC).key("d").key(ENTER, 0.6)
    r.key("E", 0.5).key(F7).key(ENTER).key(F2, 0.6)
    r.expect("TABLE EDITOR · defaults").expect("no structural changes")
    r.key(ESC).key(".").type("INSERT INTO defaults DEFAULT VALUES").key(ENTER, 0.6)
    r.key(".").type("SELECT state FROM defaults").key(ENTER, 0.6)
    r.expect("1 row(s)").expect("new")
    out.append(r)

    r = Reel("rowidentity", "Edit and delete safely with a user-defined rowid column")
    r.key(".").type("CREATE TABLE safe(rowid INTEGER, name TEXT)").key(ENTER, 0.5)
    r.type("INSERT INTO safe VALUES(7,'Alice'),(7,'Bob')").key(ENTER, 0.5)
    r.key(ESC).key("s").key(ENTER, 0.6).key(ENTER, 0.5)
    r.key(TAB).type("Alicia").key(ENTER, 0.5).expect("Alicia")
    r.key(F10, 0.5).key("x").expect("DELETE safe rowid 1")
    r.key("x", 0.5).key(".").type("SELECT name FROM safe").key(ENTER, 0.6)
    r.expect("1 row(s)").expect("Bob").expect_absent("Alicia")
    out.append(r)

    r = Reel("insertidentity", "An explicit inserted ID stays open for subsequent typing")
    r.key(".").type("CREATE TABLE gaps(id INTEGER PRIMARY KEY, name TEXT)").key(ENTER, 0.5)
    r.type("INSERT INTO gaps VALUES(1,'first'),(100,'original last')").key(ENTER, 0.5)
    r.key(ESC).key("g").key(ENTER, 0.6).key("a", 0.5)
    r.type("50").key(ENTER, 0.5).expect("EDIT gaps · 2/3")
    r.type("inserted").key(ENTER, 0.5).key(F10, 0.5)
    r.key(".").type("SELECT id,name FROM gaps WHERE id >= 50").key(ENTER, 0.6)
    r.expect("2 row(s)").expect("inserted").expect("original last")
    out.append(r)

    r = Reel("generated", "Generated columns align, stay read-only, and refresh after saves")
    r.key(".").type("CREATE TABLE derived(id INTEGER PRIMARY KEY, size INTEGER AS (length(name)), name TEXT)")
    r.key(ENTER, 0.5).type("INSERT INTO derived(name) VALUES('Alice')").key(ENTER, 0.5)
    r.key(ESC).key("d").key(ENTER, 0.6).key(ENTER, 0.5).expect("Alice")
    r.key(TAB).type("Beatrice").key(ENTER, 0.5).expect("Beatrice").expect("ƒ")
    r.key(F10, 0.5).key("a", 0.5).key(TAB).type("Charlie").key(ENTER, 0.5)
    r.expect("EDIT derived · 2/2").expect("Charlie")
    r.key(F10, 0.5).key(".").type("SELECT name,size FROM derived").key(ENTER, 0.6)
    r.expect("2 row(s)").expect("Beatrice").expect("Charlie")
    out.append(r)

    r = Reel("csvrollback", "A malformed CSV leaves no partial import or open transaction")
    r.key(".").type("CREATE TABLE csvbad(name TEXT, city TEXT)").key(ENTER, 0.5)
    r.type(f"import csvbad {WORK}/malformed.csv").key(ENTER, 0.6).expect("import: csv record")
    r.type("SELECT * FROM csvbad").key(ENTER, 0.6).expect("0 row(s)")
    r.key(".").type("BEGIN").key(ENTER, 0.5).expect("ok, 0 row(s) affected")
    r.type("ROLLBACK").key(ENTER, 0.5).expect("ok, 0 row(s) affected")
    out.append(r)

    r = Reel("sqltext", "Quoted semicolons, trigger bodies, and failed transaction batches")
    r.key(".").type("CREATE TABLE punct(id INTEGER PRIMARY KEY, name TEXT); CREATE TABLE fired(note TEXT)").key(ENTER, 0.5)
    r.type("CREATE TRIGGER punct_added AFTER INSERT ON punct BEGIN INSERT INTO fired VALUES('first;'); INSERT INTO fired VALUES('second;'); END").key(ENTER, 0.5).expect("ok")
    r.type("INSERT INTO punct VALUES(1,'Ada; O''Brien')").key(ENTER, 0.5)
    r.type("SELECT name FROM punct").key(ENTER, 0.6).expect("Ada; O'Brien")
    r.key(".").type("BEGIN; INSERT INTO punct VALUES(2,'temporary'); INSERT INTO punct VALUES(1,'duplicate'); COMMIT").key(ENTER, 0.6).expect("UNIQUE")
    r.type("ROLLBACK").key(ENTER, 0.5)
    r.type("SELECT name FROM punct ORDER BY id").key(ENTER, 0.6).expect("1 row(s)").expect("Ada; O'Brien").expect_absent("temporary")
    out.append(r)

    r = Reel("detailcrud", "Focused related records: edit, failed insert, linked add, and delete")
    r.key(".").type("CREATE TABLE zparents(id INTEGER PRIMARY KEY,name TEXT); CREATE TABLE ychildren(id INTEGER PRIMARY KEY,item TEXT NOT NULL,parent_id INTEGER REFERENCES zparents(id)); INSERT INTO zparents VALUES(1,'Ada'),(2,'Grace'); INSERT INTO ychildren VALUES(1,'modem',1),(4,'coax',1),(9,'router',2)").key(ENTER, 0.8)
    r.key(ESC).key("z").key(ENTER, 0.6).key("v", 0.8).key(TAB).key(DOWN).key(ENTER, 0.6)
    r.expect("EDIT ychildren · 2/2").expect("coax").expect_absent("EDIT zparents")
    r.key(DOWN).type("updated coax").key(ENTER, 0.7).expect("saved 1 field(s)")
    r.key(PGDN, 0.5).expect("last record").expect("EDIT ychildren · 2/2")
    r.key(F10, 0.5).key("a", 0.5).expect("NEW ychildren record")
    r.type("4").key(TAB).type("new child").key(F10, 0.7).expect("UNIQUE").expect("NEW ychildren record")
    r.key(UP).type("0").key(F10, 0.7).expect("inserted rowid 0")
    r.key("x", 0.5).expect("DELETE ychildren rowid 0").key("x", 0.6).expect("row deleted")
    r.key(".").type("SELECT name FROM zparents ORDER BY id").key(ENTER, 0.6).expect("Ada").expect("Grace").expect("2 row(s)")
    out.append(r)

    r = Reel("completeoutput", "Complete CSV, report totals, and mailing labels beyond 10,000 rows")
    r.key(".").type("CREATE TABLE outlarge(n INTEGER); WITH RECURSIVE s(n) AS (VALUES(1) UNION ALL SELECT n+1 FROM s WHERE n<10005) INSERT INTO outlarge SELECT n FROM s").key(ENTER, 0.7)
    r.type("export outlarge all.csv").key(ENTER, 1.0).expect("exported 10005 row(s)")
    r.type("export SELECT CASE WHEN n=10005 THEN abs(-9223372036854775808) ELSE n END FROM outlarge all.csv").key(ENTER, 1.0).expect("export incomplete")
    r.type("report outlarge").key(ENTER, 0.6).key(F2, 1.0).key(END, 0.5).expect("TOTAL (10005 rows)")
    r.key("w", 0.5).expect("wrote report_outlarge.txt").key(ESC, 0.5).key(ESC, 0.4)
    r.type("labels outlarge").key(ENTER, 1.0).key(END, 0.5).expect("10005")
    out.append(r)

    # Preview, revise, save/copy/rename, and reopen in a new process.
    r = Reel("builders", "builder drafts survive previews, collisions, and restart")
    r.key("c").key("Q", 0.6).keys([DOWN] * 3, gap=0.2)
    r.key(ENTER).type("> 100").key(ENTER).key(F2, 0.7).expect("5 row(s)")
    r.key(ESC, 0.5).expect('WHERE "balance" > 100')
    r.key(ENTER).type("> bad_column").key(ENTER).key(F2, 0.7)
    r.expect("no such column").expect("QUERY BY EXAMPLE")
    r.key(ENTER).type("> 100").key(ENTER).key(F6).type("draft-query").key(ENTER, 0.6)
    r.expect('saved query "draft-query"')
    r.key(F7).type("draft-copy").key(ENTER, 0.6).expect('saved query "draft-copy"')
    r.key(F8).type("draft-query").key(ENTER, 0.6).expect("already exists")
    r.key(F12, 0.4).expect("STATUS & CONNECTION").expect("reopen it")
    r.key(ESC).key(ESC).key(F8).type("draft-renamed").key(ENTER, 0.6)
    r.expect('saved query "draft-renamed"').key(ESC)
    r.key("o").key("R", 0.5).key(ENTER).type("Unsaved report title").key(ENTER)
    r.key(F2, 0.8).expect("Unsaved report title").expect("TOTAL (8 rows)")
    r.key(ESC, 0.5).expect("REPORT · orders").expect("Unsaved report title")
    r.key(F6).type("draft-report").key(ENTER, 0.6).expect('saved report "draft-report"')
    r.key(F7).type("report-copy").key(ENTER, 0.6).expect('saved report "report-copy"')
    r.key(F8).type("report-renamed").key(ENTER, 0.6).expect('saved report "report-renamed"')
    r.restart()
    r.key(".").type("qbe draft-renamed").key(ENTER, 0.6)
    r.expect("QUERY BY EXAMPLE").expect('WHERE "balance" > 100')
    r.key(F2, 0.7).expect("5 row(s)").key(ESC).key(ESC)
    r.type("report draft-report").key(ENTER, 0.5).expect("Unsaved report title")
    r.key(ESC).type("report report-renamed").key(ENTER, 0.5).expect("Unsaved report title")
    out.append(r)

    r = Reel("status80", "80-column validation, save failures, and delete confirmations", cols=80, rows=24)
    r.key("c").key(ENTER, 0.6).key("a", 0.6).key(F10, 0.5)
    r.expect('"Name" is required').expect("F12 details")
    r.key(F12, 0.5).expect("STATUS & CONNECTION").expect('"Name" is required')
    r.key(ESC).key(ENTER).type("Visible save").key(F12, 0.4).expect("STATUS & CONNECTION")
    r.key(F12, 0.4).expect("Visible save").key(F10, 0.6).expect("inserted rowid")
    r.key("x", 0.5).expect("DELETE customers rowid 9")
    r.key(F12, 0.5).expect("DELETE customers rowid 9").key(ESC).key("x", 0.6).expect("row deleted")
    r.key(".").type("INSERT INTO customers(id,name) VALUES(1,'duplicate')").key(ENTER, 0.6)
    r.expect("UNIQUE constraint failed").expect("F12 details")
    out.append(r)

    r = Reel("scrolling", "Long lists, forms, menus, and input survive terminal resizing", cols=80, rows=24)
    schema = ";".join(f"CREATE TABLE scroll{i:02}(id INTEGER)" for i in range(40))
    fields = ",".join(f"f{i:02} TEXT" + (" NOT NULL DEFAULT 'required'" if i == 34 else "") for i in range(35))
    schema += f"; CREATE TABLE zwide({fields}); INSERT INTO zwide DEFAULT VALUES"
    schema += "; INSERT INTO _phosphor_apps(name) VALUES('long_menu')"
    schema += "; WITH RECURSIVE n(x) AS (VALUES(0) UNION ALL SELECT x+1 FROM n WHERE x<39) INSERT INTO _phosphor_items(app_id,label,action_kind,action_ref,seq) SELECT (SELECT id FROM _phosphor_apps WHERE name='long_menu'), printf('Menu%02d',x), 'browse','zwide',x FROM n"
    r.key(".").key(schema).key(ENTER, 1.0).key(ESC).key(END).expect("zwide")
    r.resize(40, 12).expect("zwide").resize(100, 30).expect("zwide")
    r.key("Q", 0.5).key(END).expect("f34").resize(40, 12).expect("f34")
    r.key(ENTER).key("long filter " * 20 + "FILTERTAIL").expect("FILTERTAIL")
    r.key(F1).key(ESC).expect("FILTERTAIL").key(ESC).key(ESC)
    r.resize(80, 24).key("E", 0.5).key(END).expect("f34").resize(40, 12).expect("f34")
    r.key(ESC).key("F", 0.5).key(END).expect("f34").key(F2, 0.5)
    r.keys([TAB] * 34, gap=0.06).expect("f34:").key(F6, 0.5).key(ESC).key(ESC)
    r.resize(80, 24).key(ENTER, 0.5).key(ENTER, 0.5).key("\x1b[1;5F").expect("f34")
    r.resize(40, 12).expect("f34").type("last field saved").expect("last field saved")
    r.key(F10, 0.7).expect("saved 1 field(s)")
    r.resize(80, 24).key(".").type("SELECT f34 FROM zwide").key(ENTER, 0.5).expect("last field saved")
    r.key(".").type("apps long_menu").key(ENTER, 0.5).key(END).expect("Menu39")
    r.resize(80, 24).expect("Menu39").key(F2, 0.5).key(END).expect("Menu39")
    r.resize(40, 12).expect("Menu39").key(HOME).expect("Menu00")
    out.append(r)

    r = Reel("lookup", "Search and page through all 1,005 parents while retaining record drafts", cols=80, rows=24)
    r.key(".").key("CREATE TABLE zlookup_customers(id INTEGER PRIMARY KEY,name TEXT,city TEXT); CREATE TABLE ylookup_orders(id INTEGER PRIMARY KEY,customer_id INTEGER REFERENCES zlookup_customers(id),note TEXT); WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<1005) INSERT INTO zlookup_customers SELECT x,printf('Customer %04d',x),'Lookup City' FROM n; INSERT INTO ylookup_orders VALUES(1,1,'draft')").key(ENTER, 1.0)
    r.key(ESC).key("y").key(ENTER, 0.5).key(ENTER, 0.5).key(DOWN).key(DOWN)
    r.type("retained draft").key("\x1b[Z").key(F7, 0.7).expect("1 / 1005 matches")
    r.key(PGDN, 0.6).expect("101 / 1005 matches").expect("Customer 0101")
    r.key(END, 0.6).expect("1005 / 1005 matches").expect("Customer 1005")
    r.resize(40, 12).expect("Customer 1005").expect("Enter pick")
    r.key(ENTER).expect("retained draft").key(F10, 0.7).expect("saved 2 field(s)")
    r.resize(80, 24).key(".").type("SELECT customer_id,note FROM ylookup_orders").key(ENTER, 0.5).expect("1005").expect("retained draft")
    r.resize(80, 24).key(ESC).key(ESC).key("y").key(ENTER, 0.5).key(ENTER, 0.5).key(DOWN).key(F7, 0.7)
    r.key("/").type("missing customer").key(ENTER, 0.6).expect("No matching records")
    r.key(ENTER).expect("PICK").key("/").type("customer 0999").key(ENTER, 0.6)
    r.expect("1 / 1 matches").expect("Customer 0999").key(ENTER).key(F10, 0.7).expect("saved 1 field(s)")
    r.key(".").type("SELECT customer_id,note FROM ylookup_orders").key(ENTER, 0.5).expect("999").expect("retained draft")
    out.append(r)

    return out


def main():
    all_fail = {}
    for r in reels():
        seed()  # every reel starts from the same pristine database
        failures = r.run()
        status = "PASS" if not failures else "FAIL"
        print(f"[{status}] {r.name}: {r.title}")
        for f in failures:
            print(f"       {f}")
        if failures:
            all_fail[r.name] = failures
    if all_fail:
        print(f"\n{len(all_fail)} reel(s) failed")
        sys.exit(1)
    print("\nALL REELS PASSED")
    if "--render" in sys.argv:
        font = os.environ.get("AGG_FONT", "CaskaydiaMono Nerd Font Mono")
        for r in reels():
            cast = os.path.join(OUT, f"{r.name}.cast")
            gif = os.path.join(OUT, f"{r.name}.gif")
            subprocess.run(
                ["agg", "--font-size", "16", "--font-family", font, cast, gif],
                check=True,
                capture_output=True,
                env={**os.environ, "PATH": os.path.expanduser("~/.cargo/bin") + ":" + os.environ["PATH"]},
            )
            print(f"rendered {gif}")


if __name__ == "__main__":
    main()

#!/usr/bin/env python3
"""Full-UI test sweep, GIF-pipeline style.

Nine scripted reels cover every screen and navigation path. Each reel
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
        for ch in data[pos:]:
            self.put(ch)

    def text(self):
        return "\n".join("".join(row) for row in self.grid)


class Reel:
    def __init__(self, name, title, argv=None):
        self.name = name
        self.title = title
        self.argv = argv or [BIN, DB]
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

    def run(self):
        cast = os.path.join(OUT, f"{self.name}.cast")
        self.key(CTRL_Q, wait=0.0)
        record(self.argv, self.steps, cast, env=ENV, title=self.title, cwd=WORK)
        events = [
            json.loads(line)
            for line in open(cast).read().splitlines()[1:]
            if line
        ]
        screen = Screen(100, 30)
        failures = []
        pending = sorted(self.expects)
        i = 0
        def check(t, marker, want):
            present = marker in screen.text()
            if present != want:
                verdict = "not on screen" if want else "unexpectedly on screen"
                failures.append(f"{verdict} at t={t}: {marker!r}")

        for at, _, data in events:
            while i < len(pending) and pending[i][0] <= at:
                check(*pending[i])
                i += 1
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


def reels():
    out = []

    # ── A · navigation: seek, internals, browse motion, read-only ────
    r = Reel("nav", "navigation: seek · internals · browse · read-only")
    r.expect("… 9 internal (i)")
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
    r.key("Q", 0.6)                                   # reopen (fresh spec)
    r.keys([DOWN] * 3, gap=0.2).key(ENTER, 0.3).type("> 100").key(ENTER, 0.4)
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
    r.key(F2, 1.0).expect("region = east").expect("subtotal").expect("TOTAL (8 rows)")
    r.keys(["j"] * 3, gap=0.25)
    r.key("w", 0.6).expect("wrote report_orders.txt")
    r.key(ESC, 0.5)
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
    r.key(F6, 0.6).expect("saved form")
    r.key(F2, 0.8).expect("FORM PAINTER · orders")
    r.key(TAB, 0.4)                                   # select next field
    r.keys([RIGHT] * 3 + [DOWN] * 2, gap=0.2)
    r.key(SPACE, 0.4)                                 # place it
    r.key(UP, 0.3).key(UP, 0.3)
    r.key("t", 0.3).type("ORDER ENTRY").key(ENTER, 0.5)
    r.key("b", 0.3).keys([DOWN] * 3 + [RIGHT] * 8, gap=0.15).key("b", 0.5)
    r.key(F6, 0.6).expect("saved painted form")
    r.key(ESC, 0.4).key(ESC, 0.5)
    r.key(ENTER, 0.6)                                 # browse orders
    r.key(ENTER, 1.0).expect("ORDER ENTRY").expect("Who:")
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
    r.key("A", 0.6)
    r.keys(["j"], gap=0.3)                            # Zap sits at idx 1
    r.key("x", 0.6).expect_absent("Zap orders")       # gone
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
    out.append(r)

    # ── I · app mode: the database IS the application ────────────────
    r = Reel("appmode", "--app: menu · report · single-Esc home",
             argv=[BIN, "--app", DB])
    r.pause(1.2).expect("CRM")
    r.key("b", 1.2).expect("TOTAL (8 rows)")          # Balances report
    r.key(ESC, 0.8).expect("hotkey letters")          # ONE Esc → menu
    r.key("c", 0.8).expect("BROWSE customers")
    r.key(ESC, 0.4).key(ESC, 0.6)                     # top-level → menu
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

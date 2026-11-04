"""The host (kern/host.py): the proofs' assumptions survive contact with SQLite, a wall clock,
an outbox and HTTP. Design and caveats: docs/HOST.md."""
from __future__ import annotations

import json
import random
import threading
import urllib.error
import urllib.request

import pytest

from kern.host import Host, Unproven, make_handler, repl
from kern.run import Interp, World
from kern.syntax import parse_module, parse_world, split_maps
from kern.wf import check_module

ESCROW = check_module(parse_module(open("examples/escrow/escrow_s2.k").read()))
WORLD = "A:1000@u1 C:300@u3 E:0@u0 B:0@u2 H:0@u9"


def host(defs=ESCROW, world=WORLD, **kw):
    ticks = iter(range(1000, 10**9))
    h = Host(defs, ":memory:", clock=lambda: next(ticks), sinks=kw.pop("sinks", []), **kw)
    h.init(world)
    return h


def mem_world(world=WORLD):
    rest, maps = split_maps(world)
    return World.from_example(parse_world(rest, None), 0, maps)


def random_call(rng, defs, accounts):
    d = rng.choice(list(defs.values()))
    args = [rng.choice(accounts) if t == "Account" else rng.choice([0, 1, 2, 3, 5, 50, 99, 100, 250, -1])
            for _, t in d.params]
    return d.name, args


def test_sqlite_host_matches_the_interpreter():
    """Same calls, same clock: SQLite rollback must give exactly the interpreter's _restore semantics.
    This is what lets the proofs (made against the interpreter's model) speak about the host."""
    rng = random.Random(7)
    h, w, interp = host(), mem_world(), Interp(ESCROW)
    served = {n: d for n, d in ESCROW.items() if h.reports[n]["clean"]}
    counts = {"ok": 0, "refused": 0, "error": 0}
    events = []
    for i in range(600):
        name, args = random_call(rng, served, list(w.bal))
        r = h.call(name, args)
        w.clock, w.audit = 1000 + i, []
        outcome, w = interp.run(name, args, w)
        want = "ok" if outcome == ("ok",) else "refused"
        assert r["status"] == want, (i, name, args, r, outcome)
        if want == "refused":
            assert (r["refused"], r["args"]) == (outcome[0], list(outcome[1]))
        events += w.audit
        counts[r["status"]] += 1
    db = h.load()
    assert db.bal == w.bal
    assert db.maps == {s: m for s, m in w.maps.items() if m}
    assert [json.loads(e) for (e,) in h.db.execute("select event from outbox order by seq")] == \
        [json.loads(json.dumps(ev)) for ev in events]
    assert counts["ok"] >= 100 and counts["refused"] >= 100, counts     # not vacuous
    assert h.replay() == []


def test_refusal_writes_nothing_but_is_logged():
    h = host()
    r = h.call("transfer", ["E", "A", 5])
    assert r["status"] == "refused" and r["refused"] == "Insufficient"
    assert h.load().bal["A"] == 1000
    assert h.db.execute("select count(*) from outbox").fetchone()[0] == 0
    assert h.db.execute("select name, outcome from log").fetchall() == \
        [("transfer", json.dumps({"status": "refused", "refused": "Insufficient", "args": [5]}))]


def test_bad_call_is_an_error_not_a_refusal():
    h = host()
    assert h.call("transfer", ["A", "Nobody", 5])["status"] == "error"
    assert h.call("transfer", ["A", "B", True])["status"] == "error"        # JSON true is not an Int
    assert h.load().bal == mem_world().bal


BURN = check_module(parse_module("""\
invariant a:Account ledger.balance a >= 0

burn a:Account n:Int
body
  ledger.debit a n
"""))


def test_gate_blocks_unproven_definitions():
    h = host(BURN, "A:10@u1")
    assert not h.reports["burn"]["clean"]
    with pytest.raises(Unproven):
        h.call("burn", ["A", 5])


def test_runtime_invariant_check_rolls_back():
    """Tier ○: with the gate off, an invariant the prover did not establish is checked per call."""
    h = host(BURN, "A:10@u1", gate=False)
    assert h.call("burn", ["A", 5])["status"] == "ok"
    r = h.call("burn", ["A", 50])
    assert r["status"] == "error" and "invariant" in r["error"]
    assert h.load().bal["A"] == 5


def test_delivery_is_at_least_once():
    """A crash between delivering an event and marking it delivered sends it again.
    That is the honest guarantee; consumers must dedupe on seq."""
    got = []

    def flaky(seq, ev):
        got.append(seq)
        if len(got) == 2:
            raise RuntimeError("crash after delivering")

    h = host(sinks=[flaky])
    h.call("transfer", ["A", "B", 5])           # emits Moved: seq 1
    h.call("tip", ["A", "B", 5])                # emits Moved, Tipped: seq 2, 3
    with pytest.raises(RuntimeError):
        h.flush()
    h.sinks = [lambda seq, ev: got.append(seq)]
    assert h.flush() == 2
    assert got == [1, 2, 2, 3]                  # seq 2 delivered twice


def test_replay_detects_a_tampered_database():
    h = host()
    h.call("transfer", ["A", "B", 5])
    assert h.replay() == []
    h.db.execute("update account set balance = balance + 1 where name='B'")
    assert any("state differs" in m for m in h.replay())


def test_console_repl():
    h, out = host(), []
    repl(h, ["transfer A B 5", "transfer A B 5000", "nope", "state"], out=out.append)
    assert out[0] == "ok"
    assert out[1] == "refused Insufficient 4005"
    assert out[2].startswith("error")
    assert json.loads(out[3])["balances"]["B"] == 5


def test_http():
    from http.server import HTTPServer
    h = host()
    srv = HTTPServer(("127.0.0.1", 0), make_handler(h))
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    base = f"http://127.0.0.1:{srv.server_address[1]}"

    def post(name, body):
        req = urllib.request.Request(f"{base}/{name}", json.dumps(body).encode(), method="POST")
        try:
            with urllib.request.urlopen(req) as r:
                return r.status, json.loads(r.read())
        except urllib.error.HTTPError as e:
            return e.code, json.loads(e.read())
    try:
        assert post("transfer", {"from": "A", "to": "B", "amount": 5})[0] == 200
        code, body = post("transfer", {"from": "A", "to": "B", "amount": 5000})
        assert (code, body["refused"], body["args"]) == (409, "Insufficient", [4005])
        assert post("transfer", {"from": "A", "amount": 5})[0] == 400
        assert post("nope", {})[0] == 404
        with urllib.request.urlopen(f"{base}/manifest") as r:
            m = json.loads(r.read())
        assert m["transfer"]["uses"] == ["audit", "clock", "ledger.write"]
    finally:
        srv.shutdown()


def test_genesis_world_must_satisfy_invariants():
    h = Host(ESCROW, ":memory:", sinks=[])
    with pytest.raises(ValueError, match="invariant"):
        h.init("A:10@u1 hold.7:-1")
    assert h.init("A:10@u1")


# --- H1 (docs/research/HOST_PLAN.md): lazy world, key-scoped invariant checks

def _statements_per_call(n_accounts):
    defs = check_module(parse_module(open("examples/escrow/escrow_s2.k").read()))
    h = Host(defs, ":memory:", clock=lambda: 1, sinks=[])
    h.init(" ".join(f"A{i}:1000@u1" for i in range(n_accounts)))
    count = []
    h.db.set_trace_callback(count.append)
    assert h.call("transfer", ["A0", "A1", 5])["status"] == "ok"
    h.db.set_trace_callback(None)
    return len(count)


def test_a_call_does_not_read_the_whole_world():
    """Item 2's mechanism, without timing: the statements a call runs don't grow with the world."""
    assert _statements_per_call(50) == _statements_per_call(5000)


def test_lazy_total_sees_this_calls_writes():
    defs = check_module(parse_module("""\
pay to:Account amount:Int
body
  ledger.credit to amount
  audit.emit (Total ledger.total)
"""))
    h = host(defs, "A:10@u1 B:5@u2")
    w, interp = mem_world("A:10@u1 B:5@u2"), Interp(defs)
    r = h.call("pay", ["A", 7])
    w.clock = 1000
    interp.run("pay", ["A", 7], w)
    assert r["events"] == w.audit == [("Total", 22)]


def test_key_scoped_invariant_check_catches_a_write_at_any_key():
    """The runtime check covers the keys the call actually wrote, including computed ones."""
    defs = check_module(parse_module("""\
state hold Int
invariant o:Int hold.get o >= 0

poke id:Int
body
  hold.set (id + 1) (0 - 5)
"""))
    h = host(defs, "A:1@u1", gate=False)
    r = h.call("poke", [3])
    assert r["status"] == "error" and "invariant" in r["error"] and "at 4" in r["error"]
    assert h.load().maps == {}


@pytest.mark.parametrize("inv", ["a:Account ledger.total >= 0", "o:Int hold.get 3 >= 0",
                                 "o:Int hold.get (o + 1) >= 0", "a:Account clock.now >= 0 or ledger.balance a >= 0"])
def test_invariants_read_state_only_at_their_own_key(inv):
    """Checking invariants key by key is only sound if they read nothing else."""
    from kern.wf import WellFormedError
    with pytest.raises(WellFormedError, match="must read state at"):
        check_module(parse_module(f"state hold Int\ninvariant {inv}\n"))


# --- H2: named output channels

from kern.report import analyse  # noqa: E402
from kern.syntax import ParseError  # noqa: E402
from kern.wf import WellFormedError  # noqa: E402

CH = """\
channel console

hello to:Account amount:Int
  need amount > 0 else BadAmount
  ok otherwise
  ensure ledger.balance' to = ledger.balance to + amount
  emit console Line to amount
  emit Credited to amount
body
  ledger.credit to amount
  audit.emit (Credited to amount)
  console.emit (Line to amount)
"""


def _emit_goals(src, name):
    defs = check_module(parse_module(src))
    return [g for g in analyse(defs, mutation=False)[name]["guarantees"] if g["kind"] == "emit"]


def test_channel_spec_is_an_exact_sequence_per_channel():
    assert [g["status"] for g in _emit_goals(CH, "hello")] == ["proven", "proven"]
    wrong = _emit_goals(CH.replace("console.emit (Line to amount)", "console.emit (Line to 0)"), "hello")
    g = [g for g in wrong if g["clause"].startswith("console")][0]
    assert g["status"] == "refuted" and g["counterexample"]["confirmed_by_interpreter"]
    assert [g["status"] for g in wrong if not g["clause"].startswith("console")] == ["proven"]   # audit unaffected


def test_a_channel_the_callee_spec_omits_is_opaque_to_callers():
    src = CH.replace("  emit console Line to amount\n", "") + """
twice to:Account amount:Int
  emit console Line to amount
body
  hello to amount
"""
    g = [g for g in _emit_goals(src, "twice") if g["clause"].startswith("console")][0]
    assert g["status"] in ("unknown", "tested") and "does not say what it emits" in g["reason"]


@pytest.mark.parametrize("src,err", [
    ("f a:Account\n  emit radio Ping a\nbody\n  ledger.credit a 1\n", WellFormedError),
    ("channel audit\n", ParseError),
    ("channel console\n\nf a:Account\n  ensure console.emit (Ping a)\nbody\n  console.emit (Ping a)\n", WellFormedError),
])
def test_channels_must_be_declared_and_are_not_readable(src, err):
    with pytest.raises(err):
        check_module(parse_module(src))


def test_audit_only_code_keeps_its_hash():
    """Item 3: channels change nothing for existing code (hash of escrow_s1 transfer, before H2)."""
    assert parse_module(open("examples/escrow/escrow_s1.k").read())["transfer"].hash == "27367a3f"


def test_host_delivers_channels_after_commit_and_replays_them():
    defs = check_module(parse_module(CH))
    lines, audit = [], []
    h = host(defs, "A:0@u1", sinks=[lambda s, e: audit.append(e)],
             channel_sinks={"console": [lambda s, e: lines.append(e)]})
    assert h.call("hello", ["A", 5])["out"] == {"console": [("Line", "A", 5)]}
    assert h.call("hello", ["A", 0])["status"] == "refused"          # rolled back: nothing queued
    assert lines == [] and h.flush() == 2
    assert lines == [("Line", "A", 5)] and audit == [("Credited", "A", 5)]
    assert h.db.execute("select channel from outbox order by seq").fetchall() == [("audit",), ("console",)]
    assert h.replay() == []
    h.db.execute("update outbox set event = ? where channel = 'console'", (json.dumps(["Line", "A", 6]),))
    assert any("outbox differs" in m or "differs" in m for m in h.replay())


# --- H3/H4: requests are events, replies are calls (examples/limits/limits.k, limits.host)

from kern.host import ConfigError, HttpNet, SimNet, load_config, manifest_text  # noqa: E402

LIMITS = check_module(parse_module(open("examples/limits/limits.k").read()))
LIMITS_REPORTS = analyse(LIMITS, mutation=False)
CONFIG = "Fetch -> GET http://127.0.0.1:{port}/limit/{1}; reply got_limit {1} {2} $.value; fail limit_failed {1} {2}"
LIMITS_WORLD = "A:500@u1 B:500@u2 C:500@u3 K:0@u9"


def limits_host(net, port=1, **kw):
    eps = load_config(CONFIG.replace("{port}", str(port)), LIMITS)
    ticks = iter(range(1000, 10**9))
    h = Host(LIMITS, ":memory:", clock=lambda: next(ticks), sinks=[], reports=LIMITS_REPORTS,
             endpoints=eps, net=net(eps) if callable(net) and not isinstance(net, HttpNet) else net, **kw)
    h.init(LIMITS_WORLD)
    return h


def test_hostile_network_cannot_break_the_invariants():
    """Item 5: 1000 seeded runs; replies dropped, duplicated, delayed, reordered, failed or garbled."""
    stats = {"runs": 0, "calls": 0, "replies": 0, "applied": 0, "dup_or_old": 0, "dup_refused": 0,
             "faults": 0, "replay_mismatch": 0, "stuck_pending": 0}
    for run in range(1000):
        rng = random.Random(run)
        values = lambda ev, rng=rng: rng.choice([{"value": rng.randint(0, 900)}, {"value": -1}, {"value": "x"}, {}])
        h = limits_host(lambda eps, run=run: SimNet(eps, values, seed=run))
        applied, latest = set(), {}
        for _ in range(25):
            a = rng.choice("ABC")
            op = rng.random()
            if op < 0.35:
                r = h.call("request_limit", [a])
                if r["status"] == "ok":
                    latest[a] = latest.get(a, 0) + 1
            elif op < 0.65:
                r = h.call("withdraw", [a, "K", rng.choice([1, 50, 200, 600])])
            elif op < 0.75:
                r = h.call("reset_day", [a])
            else:
                r = {"status": "ok"}
            stats["calls"] += 1
            stats["faults"] += r.get("fault") == "module"
            h.flush()
            for name, args, res in h.pump(h.net.release() if rng.random() < 0.5 else []):
                stats["replies"] += 1
                stats["faults"] += res.get("fault") == "module"
                key = (args[0], args[1])
                if key in applied or args[1] != latest.get(args[0]):
                    stats["dup_or_old"] += 1
                    stats["dup_refused"] += res.get("refused") == "Stale"
                elif res["status"] == "ok":
                    applied.add(key)
                    stats["applied"] += 1
        stats["replay_mismatch"] += bool(h.replay())
        stats["stuck_pending"] += sum(h.load().maps.get("pending", {}).values())   # dropped replies: no timeout
        stats["runs"] += 1
    print(stats)
    assert stats["faults"] == 0 and stats["replay_mismatch"] == 0
    assert stats["dup_or_old"] > 500 and stats["dup_refused"] == stats["dup_or_old"]
    assert stats["applied"] > 1000                                     # not vacuous


def test_config_is_checked_at_startup():
    """Item 6: a request with no endpoint, an endpoint nobody sends, or a reply def with the wrong
    types are refused before anything runs."""
    with pytest.raises(ConfigError, match="no host config"):
        Host(LIMITS, ":memory:", sinks=[], reports=LIMITS_REPORTS)
    with pytest.raises(ConfigError, match="has no endpoint"):
        load_config("", LIMITS)
    with pytest.raises(ConfigError, match="never sends"):
        load_config(CONFIG + "\nPing -> GET http://x/; reply got_limit {1} {2} $.v; fail limit_failed {1} {2}", LIMITS)
    with pytest.raises(ConfigError, match="takes"):
        load_config("Fetch -> GET http://x/{1}; reply got_limit {1} $.value; fail limit_failed {1} {2}", LIMITS)
    with pytest.raises(ConfigError, match="does not exist"):
        load_config("Fetch -> GET http://x/{1}; reply nope {1}; fail limit_failed {1} {2}", LIMITS)


def test_manifest_lists_every_endpoint():
    h = limits_host(lambda eps: SimNet(eps, lambda ev: {"value": 1}))
    assert "net: Fetch -> GET http://127.0.0.1:1/limit/*  reply got_limit, fail limit_failed" in manifest_text(h)


def test_real_http_reply_and_failure():
    """One run against a real local server: a 200 sets the limit, a 500 becomes limit_failed."""
    from http.server import BaseHTTPRequestHandler, HTTPServer

    class Risk(BaseHTTPRequestHandler):
        def do_GET(self):
            if self.path == "/limit/A":
                body, code = b'{"value": 300}', 200
            else:
                body, code = b"no", 500
            self.send_response(code)
            self.send_header("content-length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def log_message(self, *a):
            pass

    srv = HTTPServer(("127.0.0.1", 0), Risk)
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    try:
        h = limits_host(None, port=srv.server_address[1])
        assert isinstance(h.net, HttpNet)
        h.call("request_limit", ["A"])
        h.call("request_limit", ["B"])
        h.flush()
        results = {name: res["status"] for name, args, res in h.pump()}
        assert results == {"got_limit": "ok", "limit_failed": "ok"}
        w = h.load()
        assert w.maps["limit"]["A"] == 300 and w.maps["pending"] == {"A": 0, "B": 0}
        assert h.call("withdraw", ["A", "K", 200])["status"] == "ok"
        assert h.call("withdraw", ["B", "K", 1])["refused"] == "OverLimit"
        assert h.replay() == []
    finally:
        srv.shutdown()



def _risk_server():
    """A local risk service: /limit/A answers {"value": 300}, anything else a 500."""
    from http.server import BaseHTTPRequestHandler, HTTPServer

    class Risk(BaseHTTPRequestHandler):
        def do_GET(self):
            body, code = (b'{"value": 300}', 200) if self.path == "/limit/A" else (b"no", 500)
            self.send_response(code)
            self.send_header("content-length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def log_message(self, *a):
            pass
    srv = HTTPServer(("127.0.0.1", 0), Risk)
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    return srv


def test_serve_runs_network_replies():
    """kern-44's R2: under `serve`, a request's reply runs as a new call; before, it waited in the inbox forever."""
    from http.server import HTTPServer
    risk = _risk_server()
    h = limits_host(None, port=risk.server_address[1])
    srv = HTTPServer(("127.0.0.1", 0), make_handler(h))
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    base = f"http://127.0.0.1:{srv.server_address[1]}"
    try:
        for a in ("A", "B"):
            req = urllib.request.Request(f"{base}/request_limit", json.dumps({"a": a}).encode(), method="POST")
            with urllib.request.urlopen(req) as r:
                assert r.status == 200
        with urllib.request.urlopen(f"{base}/state") as r:             # served after the replies ran
            st = json.loads(r.read())["state"]
        assert st["limit"] == {"A": 300} and st["pending"] == {"A": 0, "B": 0}
        names = [row[0] for row in h.db.execute("select name from log order by seq")]
        assert names[-4:] == ["request_limit", "got_limit", "request_limit", "limit_failed"]
        assert h.replay() == []
    finally:
        srv.shutdown()
        risk.shutdown()


def test_repl_runs_network_replies_and_says_so():
    risk = _risk_server()
    try:
        h, out = limits_host(None, port=risk.server_address[1]), []
        repl(h, ["request_limit A", "request_limit B", "withdraw A K 200"], out=out.append)
        assert out == ["ok", "reply got_limit A 1 300 -> ok", "ok", "reply limit_failed B 1 -> ok", "ok"]
    finally:
        risk.shutdown()


def test_a_new_database_without_world_gets_an_empty_genesis(tmp_path):
    """kern-44: `repl`/`serve --db X.db` with no --world recorded no genesis, so `replay` said "no genesis world"."""
    import subprocess
    import sys
    db = str(tmp_path / "x.db")

    def host_cli(*args, stdin=""):
        return subprocess.run([sys.executable, "-m", "kern.host", *args], input=stdin, capture_output=True, text=True)
    assert host_cli("repl", "examples/shop", "--db", db, stdin="restock 1 2\n").stdout.strip() == "ok"
    assert host_cli("replay", "examples/shop", "--db", db).stdout.strip() == "replay of 1 logged calls matches the database"
    db2 = str(tmp_path / "y.db")
    assert host_cli("repl", "examples/shop", "--db", db2, "--world", "", stdin="restock 1 2\n").returncode == 0
    assert "matches the database" in host_cli("replay", "examples/shop", "--db", db2).stdout

# --- item 8: crash recovery (a real process, killed with SIGKILL during a flush)

CHILD = r"""
import os, sys, time
sys.path.insert(0, os.getcwd())
from kern.host import Host
from kern.syntax import parse_module
from kern.wf import check_module
db, log = sys.argv[1], sys.argv[2]
def slow(seq, ev):
    with open(log, "a") as f:
        f.write(f"{seq}\n"); f.flush(); os.fsync(f.fileno())
    time.sleep(0.02)
defs = check_module(parse_module(open("examples/escrow/escrow_s2.k").read()))
Host(defs, db, sinks=[slow]).flush()
"""


def test_kill_during_flush_redelivers_with_at_most_one_duplicate_per_kill(tmp_path):
    import os
    import signal
    import subprocess
    import sys
    import time
    db, log = str(tmp_path / "e.db"), str(tmp_path / "delivered.txt")
    h = Host(ESCROW, db, clock=lambda: 1, sinks=[])
    h.init(WORLD)
    for _ in range(40):
        assert h.call("transfer", ["A", "B", 1])["status"] == "ok"      # 40 committed, undelivered events
    h.db.close()
    script = tmp_path / "child.py"
    script.write_text(CHILD)
    lines = lambda: open(log).read().split() if os.path.exists(log) else []
    kills = 0
    for stop_after in (5, 15, 27):                                     # kill -9 three times mid-flush
        p = subprocess.Popen([sys.executable, str(script), db, log], cwd=os.getcwd())
        while len(lines()) < stop_after and p.poll() is None:
            time.sleep(0.005)
        p.send_signal(signal.SIGKILL)
        p.wait()
        kills += 1
    subprocess.run([sys.executable, str(script), db, log], cwd=os.getcwd(), check=True)   # restart, finish
    got = [int(x) for x in lines()]
    dups = len(got) - len(set(got))
    assert sorted(set(got)) == list(range(1, 41))                      # every committed event delivered
    assert dups <= kills, (dups, kills)
    print({"events": 40, "kills": kills, "deliveries": len(got), "duplicates": dups})


# --- H5: the proof page says what the host assumes, as assumed

def test_proof_page_host_section_is_assumed_not_proven():
    from kern import render
    from kern.host import host_facts
    h = limits_host(lambda eps: SimNet(eps, lambda ev: {"value": 1}))
    section = render.host_html(host_facts(h))
    assert "assumed, not proven" in section and "✔" not in section
    assert "GET <code>http://127.0.0.1:1/limit/{1}</code>" in section and "at least once" in section
    assert section.count("badge warn'>assumed</span>") == len(host_facts(h)["assumptions"])
    plain = render.host_html(host_facts(host()))                   # escrow: no network
    assert "Network endpoints" not in plain and "network reply" not in plain


# --- S7 (docs/research/ROADMAP_RUN.md): proven checks removed at runtime

def test_runtime_checks_are_on_by_default_and_skip_proven_removes_proven_ones():
    """Fredrik's decision (2026-11-04): checks on by default; skipping proven ones is opt-in."""
    assert host().manifest()["transfer"]["runtime_invariant_check"] == "on"
    assert "on for every definition (the default" in manifest_text(host())
    h = host(skip_proven=True)
    assert h.manifest()["transfer"]["runtime_invariant_check"] == "removed (proven)"
    assert "--skip-proven" in manifest_text(h) and "sampled audit rate" in manifest_text(h)
    b = host(BURN, "A:10@u1", gate=False, skip_proven=True)   # unproven: still checked at runtime
    assert b.manifest()["burn"]["runtime_invariant_check"] == "on"
    assert "invariant" in b.call("burn", ["A", 50])["error"]


def test_the_default_catches_an_invariant_broken_outside_kern_and_skip_proven_does_not():
    """A database edited outside Kern while the host runs breaks the induction the proof relies on."""
    for skip, want in ((False, "error"), (True, "ok")):
        h = host(ESCROW, WORLD, skip_proven=skip)
        h.db.execute("update account set balance = -5 where name='B'")      # outside Kern
        r = h.call("transfer", ["A", "B", 1])                               # B ends at -4
        assert r["status"] == want, (skip, r)


def test_startup_audit_refuses_a_database_broken_outside_kern(tmp_path):
    db = str(tmp_path / "e.db")
    h = Host(ESCROW, db, clock=lambda: 1, sinks=[])
    h.init(WORLD)
    h.db.execute("update account set balance = -5 where name='B'")         # e.g. another process
    h.db.close()
    with pytest.raises(ConfigError, match="startup audit"):
        Host(ESCROW, db, sinks=[])


def test_sampled_audit_runs_skipped_checks():
    h = host(skip_proven=True, audit_rate=1.0)
    for _ in range(5):
        h.call("transfer", ["A", "B", 1])
    assert h.audit_report()["sampled"] == 5 and h.audit_report()["violations"] == 0


def test_a_provider_with_no_value_fails_the_call_atomically():
    """FZ finding 2 in the host: secrets.randbelow(0) must be a call error, not a crash."""
    from kern.syntax import parse_module as _pm
    from kern.wf import check_module as _cm
    src = """handler rand.below n:Int -> Int
  ensure result >= 0 and result < n

state hold Int

pick n:Int
  ensure hold.get' 0 >= 0
body
  hold.set 0 9
  r = rand.below n
  hold.set 0 r
"""
    h = host(_cm(_pm(src)), "A:1@u1", gate=False)
    r = h.call("pick", [0])
    assert r["status"] == "error" and "gave no value" in r["error"]
    assert h.load().maps.get("hold", {}).get(0, 0) == 0                     # nothing was written

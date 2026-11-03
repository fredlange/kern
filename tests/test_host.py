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

ESCROW = check_module(parse_module(open("examples/escrow_s2.k").read()))
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


# --- H1 (docs/HOST_PLAN.md): lazy world, key-scoped invariant checks

def _statements_per_call(n_accounts):
    defs = check_module(parse_module(open("examples/escrow_s2.k").read()))
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

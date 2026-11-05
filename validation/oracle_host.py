"""Host oracle for the Rust port: kern/host.py run through scripted scenarios, so toolchain/src/host.rs can be
checked against it op for op, and each host's database can be replayed by the other.

    .venv/bin/python validation/oracle_host.py OUT.json             record the scenarios (databases in build/host_py/)
    .venv/bin/python validation/oracle_host.py --replay DIR          Python's replay of every database in DIR
                                                                     (DIR = build/host_rs: the Rust host's databases)

A scenario is a module, a genesis world and a list of ops: ["call", name, args], ["flush"], ["pump"], ["release"]
(pump the replies SimNet held back), ["replay"], ["state"], ["sql", statement] (tampering). Every op's result is
recorded as JSON (exceptions as ["exception", type, text]), with every sink delivery and, at the end, every row of
every table. The clock ticks from 1000; gate=False (the gate needs the report, compared elsewhere); assumed
handlers get a deterministic provider; the network is SimNet with a deterministic answer.
"""
import json
import os
import random
import sqlite3
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, ROOT)

from kern import package  # noqa: E402
from kern.host import Host, SimNet, load_config  # noqa: E402

TABLES = ["meta", "account", "cell", "log", "outbox"]
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
"""  # the channels module of toolchain/tests/host.rs (CH), formerly read from tests/test_host.py
RAFFLE = os.path.join(ROOT, "examples/raffle/raffle.k")


def answers():
    """SimNet's well-behaved server, deterministic: a counter cycles through good and garbled bodies."""
    n = [0]

    def answer(ev):
        n[0] += 1
        return [{"value": (n[0] * 37) % 901}, {"value": -1}, {"value": "x"}, {}, {"value": True}][n[0] % 5]
    return answer


def provider():
    n = [0]

    def below(args):
        n[0] += 1
        return (n[0] * 7) % max(args[0], 1) if args[0] > 0 else (_ for _ in ()).throw(ValueError("Upper bound must be positive."))
    return below


def random_calls(defs, accounts, n, seed, extra=()):
    rng = random.Random(seed)
    ops = []
    for i in range(n):
        d = rng.choice(list(defs.values()))
        args = [rng.choice(accounts) if t == "Account" else rng.choice([0, 1, 2, 3, 5, 50, 99, 100, 250, -1])
                for _, t in d.params]
        ops.append(["call", d.name, args])
        if i % 7 == 0:
            ops.append(["flush"])
    ops += [["call", "nope", []], ["call", list(defs)[0], ["A"] * 9], *extra, ["flush"], ["replay"], ["state"]]
    return ops


def scenarios():
    escrow = os.path.join(ROOT, "examples/escrow/escrow_s2.k")
    edefs = package.load(escrow)
    out = [{"name": "escrow", "path": escrow, "world": "A:1000@u1 C:300@u3 E:0@u0 B:0@u2 H:0@u9",
            "ops": random_calls(edefs, ["A", "B", "C", "E", "H", "Z"], 400, 7,
                                [["sql", "update account set balance = balance + 1 where name = 'A'"]])}]
    for name in ("ledger-tour/ledger_v3.k", "ledger-tour/ledger_v7.k", "batch/batch.k", "shop", "library"):
        path = os.path.join(ROOT, "examples", name)
        defs = package.load(path)
        accounts = ["A", "B", "C", "H"]
        world = "A:500@u1 B:300@u2 C:0@u1 H:0@u9" + (" stock.1:5 price.1:7 stock.2:0" if name == "shop" else "")
        out.append({"name": name.replace("/", "_"), "path": path, "world": world,
                    "ops": random_calls(defs, accounts, 150, 11)})
    ch = os.path.join(ROOT, "build", "host_ch.k")
    open(ch, "w").write(CH)
    out.append({"name": "channels", "path": ch, "world": "A:0@u1",
                "ops": [["call", "hello", ["A", 5]], ["call", "hello", ["A", 0]], ["flush"], ["replay"],
                        ["sql", "update outbox set event = '[\"Line\", \"A\", 6]' where channel = 'console'"],
                        ["replay"]]})
    rdefs = package.load(RAFFLE)
    out.append({"name": "raffle", "path": RAFFLE, "world": "A:500@u1 B:300@u2 C:0@u1 H:0@u9",
                "ops": random_calls(rdefs, ["A", "B", "C", "H"], 120, 13)})
    limits = os.path.join(ROOT, "examples/limits/limits.k")
    for seed in range(6):
        rng = random.Random(seed)
        ops = []
        for _ in range(40):
            a, op = rng.choice("ABC"), rng.random()
            if op < 0.35:
                ops.append(["call", "request_limit", [a]])
            elif op < 0.65:
                ops.append(["call", "withdraw", [a, "K", rng.choice([1, 50, 200, 600])]])
            elif op < 0.75:
                ops.append(["call", "reset_day", [a]])
            ops.append(["flush"])
            ops.append(["release"] if rng.random() < 0.5 else ["pump"])
        ops += [["replay"], ["state"]]
        out.append({"name": f"limits{seed}", "path": limits, "world": "A:500@u1 B:500@u2 C:500@u3 K:0@u9",
                    "config": "Fetch -> GET http://127.0.0.1:1/limit/{1}; reply got_limit {1} {2} $.value; "
                              "fail limit_failed {1} {2}", "seed": seed, "ops": ops})
    return out


def j(x):
    return json.loads(json.dumps(x))


def run(sc, db):
    defs = package.load(sc["path"])
    ticks = iter(range(1000, 10**9))
    delivered = []
    sink = lambda ch: (lambda seq, ev: delivered.append([seq, ch, j(ev)]))  # noqa: E731
    kw = {}
    if "config" in sc:
        eps = load_config(sc["config"], defs)
        kw = {"endpoints": eps, "net": SimNet(eps, answers(), seed=sc["seed"])}
    if os.path.exists(db):
        os.unlink(db)
    h = Host(defs, db, clock=lambda: next(ticks), sinks=[sink("audit")],
             channel_sinks={c: [sink(c)] for c in defs.channels}, gate=False,
             providers={"rand.below": provider()}, **kw)
    h.init(sc["world"])
    results = []
    for op in sc["ops"]:
        try:
            if op[0] == "call":
                r = h.call(op[1], op[2])
            elif op[0] == "flush":
                r = h.flush()
            elif op[0] == "pump":
                r = h.pump()
            elif op[0] == "release":
                r = h.pump(h.net.release())
            elif op[0] == "replay":
                r = h.replay()
            elif op[0] == "state":
                w = h.load()
                r = {"bal": w.bal, "own": w.own, "maps": [[s, list(m.items())] for s, m in w.maps.items()]}
            elif op[0] == "sql":
                h.db.execute(op[1])
                r = None
            results.append(j(r))
        except Exception as e:  # noqa: BLE001
            results.append(["exception", type(e).__name__, str(e)])
    con = sqlite3.connect(db)
    tables = {t: [list(row) for row in con.execute(f"select * from {t} order by rowid")] for t in TABLES}
    return {"results": results, "delivered": delivered, "tables": tables}


HTTP = [["GET", "/state", None], ["GET", "/nope", None], ["POST", "/nope", "{}"],
        ["POST", "/transfer", '{"from": "A", "to": "B", "amount": 5}'],
        ["POST", "/transfer", '{"from": "A", "to": "B", "amount": 5000}'],
        ["POST", "/transfer", '{"from": "A", "to": "Z", "amount": 5}'],
        ["POST", "/transfer", '{"from": "A", "to": "B", "amount": "x"}'],
        ["POST", "/transfer", '{"from": "A", "to": "B"}'], ["POST", "/transfer", "not json"],
        ["POST", "/transfer", '[1, 2]'], ["POST", "/transfer", ""], ["GET", "/state", None]]


def http(sc):
    """The HTTP front end: each request's status and exact body bytes."""
    import threading
    import urllib.error
    import urllib.request
    from http.server import HTTPServer

    from kern.host import make_handler
    defs = package.load(sc["path"])
    ticks = iter(range(1000, 10**9))
    h = Host(defs, ":memory:", clock=lambda: next(ticks), sinks=[], gate=False)
    h.init(sc["world"])
    srv = HTTPServer(("127.0.0.1", 0), make_handler(h))
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    out = []
    for method, path, body in HTTP:
        req = urllib.request.Request(f"http://127.0.0.1:{srv.server_address[1]}{path}", method=method,
                                     data=None if body is None else body.encode())
        try:
            with urllib.request.urlopen(req) as r:
                out.append([r.status, r.read().decode()])
        except urllib.error.HTTPError as e:
            out.append([e.code, e.read().decode()])
    srv.shutdown()
    return out


def replay_dir(d):
    ok = True
    for sc in scenarios():
        db = os.path.join(d, sc["name"] + ".db")
        if not os.path.exists(db):
            print(f"{sc['name']}: no database in {d}")
            ok = False
            continue
        defs = package.load(sc["path"])
        kw = {}
        if "config" in sc:
            eps = load_config(sc["config"], defs)
            kw = {"endpoints": eps, "net": SimNet(eps, answers(), seed=0)}
        try:
            h = Host(defs, db, sinks=[], gate=False, providers={"rand.below": provider()}, **kw)
            bad = h.replay()
        except Exception as e:  # noqa: BLE001  (a tampered database may fail the startup audit)
            bad = [f"{type(e).__name__}: {e}"]
        print(f"{sc['name']}: Python replay of the Rust database: {bad or 'matches'}")
    return ok


def main(argv):
    if argv and argv[0] == "--replay":
        return 0 if replay_dir(argv[1]) else 1
    dbs = os.path.join(ROOT, "build", "host_py")
    os.makedirs(dbs, exist_ok=True)
    out = []
    for sc in scenarios():
        rec = run(sc, os.path.join(dbs, sc["name"] + ".db"))
        out.append({**sc, **rec})
        print(f"{sc['name']}: {len(sc['ops'])} ops, {sum(1 for r in rec['results'] if isinstance(r, list) and r[:1] == ['exception'])} "
              f"exceptions, {len(rec['delivered'])} deliveries", file=sys.stderr)
    sc = {"name": "http", "path": os.path.join(ROOT, "examples/escrow/escrow_s2.k"),
          "world": "A:1000@u1 C:300@u3 E:0@u0 B:0@u2 H:0@u9", "http": HTTP}
    out.append({**sc, "responses": http(sc)})
    with open(argv[0], "w") as f:
        json.dump(out, f, ensure_ascii=False)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))

"""A host for Kern modules: the boundary between verified code and the operating system.

Kern code never touches the OS. The host does, and keeps the three properties the proofs
rest on (see docs/HOST.md):

  atomicity    one SQLite transaction per top-level call; a refusal or a runtime error
               writes no state and emits nothing
  closed world the module can only reach the handlers in run.HANDLERS; the host maps them:
               ledger + `state` maps -> SQLite tables (owned state, rolled back)
               clock.now             -> wall clock, read once per call and recorded (an observation)
               audit.emit            -> an outbox table, delivered to sinks after commit (an action)
  determinism  every attempted call is logged with its recorded `now`; `replay` re-runs the
               log from the genesis world and must reach the same state and outbox

Delivery is at-least-once: a crash between delivering an outbox event and marking it
delivered sends it again. Kern cannot prove anything about that; consumers need the event's
`seq` as an idempotency key.

  python -m kern.host serve    FILE.k --db FILE.db [--world "A:500@u1 ..."] [--port 8080]
  python -m kern.host repl     FILE.k --db FILE.db [--world ...]     (calls on stdin, outbox on stdout)
  python -m kern.host replay   FILE.k --db FILE.db
  python -m kern.host manifest FILE.k [--db FILE.db]
"""
from __future__ import annotations

import argparse
import json
import random
import sqlite3
import sys
import threading
import time
from http.server import BaseHTTPRequestHandler, HTTPServer

from .infer import fails, uses
from .report import analyse
from .run import Interp, KernError, World
from .syntax import ParseError, parse_module, parse_world, split_maps
from .wf import check_module

SCHEMA = """
create table if not exists meta(k text primary key, v text not null);
create table if not exists account(name text primary key, balance integer not null, owner text not null);
create table if not exists cell(state text not null, key text not null, value integer not null,
                                primary key(state, key));
create table if not exists log(seq integer primary key, name text not null, args text not null,
                               now integer not null, outcome text not null, observed text not null default '[]');
create table if not exists outbox(seq integer primary key, call integer not null, event text not null,
                                  delivered integer not null default 0, channel text not null default 'audit');
"""


class Unproven(Exception):
    pass


class ConfigError(Exception):
    """The host config doesn't match the module; raised at startup, never during a call."""


# --- the network: requests are `net` events, replies come back as new top-level calls

def net_requests(defs):
    """Constructor -> argument types, for every event a module can send on `net` (bodies and specs)."""
    from .syntax import flat
    out = {}
    for d in defs.values():
        types = dict(d.params)
        for s in flat(d.body):
            if s[0] == "do" and s[1][0] == "app" and s[1][1][0] == "net.emit":
                ev = s[1][2][0]
                ctor, args = (ev[1][0], ev[2]) if ev[0] == "app" else (ev[1], [])
                out.setdefault(ctor, [types.get(a[1], "Int") if a[0] == "name" else "Int" for a in args])
        for i, (ctor, args, *_) in enumerate(d.emits):
            if d.emit_channel.get(i) == "net":
                out.setdefault(ctor, [types.get(a[1], "Int") if a[0] == "name" else "Int" for a in args])
    return out


class Endpoint:
    """`Ctor -> GET URL; reply DEF ARGS; fail DEF ARGS`. ARGS are `{i}` (the event's i-th argument)
    or `$.field` (an Int field of the JSON reply). Kern never sees the bytes."""

    def __init__(self, line, no):
        try:
            head, reply, fail = [p.strip() for p in line.split(";")]
            ctor, target = [p.strip() for p in head.split("->")]
            self.method, self.url = target.split()
            kw, self.reply, *self.reply_args = reply.split()
            kw2, self.fail, *self.fail_args = fail.split()
            assert kw == "reply" and kw2 == "fail" and self.method in ("GET", "POST")
        except (ValueError, AssertionError):
            raise ConfigError(f"line {no}: want `Ctor -> GET URL; reply DEF ARGS; fail DEF ARGS`")
        self.ctor, self.src = ctor, line.strip()

    def args_for(self, ev, body, which):
        out = []
        for a in (self.reply_args if which == "reply" else self.fail_args):
            if a.startswith("{"):
                out.append(ev[int(a[1:-1])])
            else:
                v = body
                for k in a[2:].split("."):
                    v = v[k]
                if not isinstance(v, int) or isinstance(v, bool):
                    raise ValueError(f"{a} is not an Int")
                out.append(v)
        return out

    def url_for(self, ev):
        url = self.url
        for i, a in enumerate(ev[1:], 1):
            url = url.replace("{%d}" % i, str(a))
        return url


def load_config(text, defs):
    """Parse and validate a `.host` file against the module: every request constructor the module
    can send needs exactly one endpoint, and reply/fail definitions must exist with matching types."""
    eps, problems = {}, []
    for no, line in enumerate(text.splitlines(), 1):
        if line.strip() and not line.strip().startswith("#"):
            ep = Endpoint(line, no)
            eps[ep.ctor] = ep
    reqs = net_requests(defs)
    for ctor, types in reqs.items():
        if ctor not in eps:
            problems.append(f"request {ctor} has no endpoint in the host config")
    for ctor, ep in eps.items():
        if ctor not in reqs:
            problems.append(f"endpoint for {ctor}, which the module never sends")
            continue
        for which, name, args in (("reply", ep.reply, ep.reply_args), ("fail", ep.fail, ep.fail_args)):
            d = defs.get(name)
            if d is None:
                problems.append(f"{ctor}: {which} definition {name!r} does not exist")
                continue
            got = [reqs[ctor][int(a[1:-1]) - 1] if a.startswith("{") else "Int" for a in args]
            want = [t for _, t in d.params]
            if got != want:
                problems.append(f"{ctor}: {which} {name} takes {want}, the config gives {got}")
    if problems:
        raise ConfigError("; ".join(problems))
    return eps


class HttpNet:
    """Real requests. A timeout, a non-2xx status or a decode error becomes the `fail` call."""

    def __init__(self, endpoints, timeout=2.0):
        self.endpoints, self.timeout = endpoints, timeout

    def __call__(self, seq, ev):
        import urllib.request
        ep = self.endpoints[ev[0]]
        try:
            req = urllib.request.Request(ep.url_for(ev), method=ep.method)
            with urllib.request.urlopen(req, timeout=self.timeout) as r:
                body = json.loads(r.read())
            return [(ep.reply, ep.args_for(ev, body, "reply"))]
        except Exception:
            return [(ep.fail, ep.args_for(ev, None, "fail"))]


class SimNet:
    """A hostile network for tests, seeded: each request's reply may be dropped, duplicated,
    delayed (held back and released later, out of order) or failed. `answer(ev)` gives the
    JSON body a well-behaved server would send."""

    def __init__(self, endpoints, answer, seed=0, p_drop=0.15, p_dup=0.2, p_delay=0.3, p_fail=0.1):
        self.endpoints, self.answer, self.rng = endpoints, answer, random.Random(seed)
        self.p = (p_drop, p_dup, p_delay, p_fail)
        self.held = []

    def __call__(self, seq, ev):
        ep, r = self.endpoints[ev[0]], self.rng.random
        p_drop, p_dup, p_delay, p_fail = self.p
        if r() < p_drop:
            return []
        failed = r() < p_fail
        try:
            call = (ep.fail, ep.args_for(ev, None, "fail")) if failed else \
                (ep.reply, ep.args_for(ev, self.answer(ev), "reply"))
        except (KeyError, TypeError, ValueError):            # an undecodable reply is a failure
            call = (ep.fail, ep.args_for(ev, None, "fail"))
        calls = [call, call] if r() < p_dup else [call]
        if r() < p_delay:
            self.held += calls
            return []
        return calls

    def release(self):
        """Deliver some held-back replies, in a random order."""
        self.rng.shuffle(self.held)
        k = self.rng.randint(0, len(self.held))
        out, self.held = self.held[:k], self.held[k:]
        return out


class _Lazy:
    """One store (balances, owners, or one `state` map) read from SQLite on demand.

    `cache` holds values read in this call and is shared by copies (the transaction makes it
    valid for the whole call). `dirty` holds what the call wrote; a copy copies only that, so
    Interp.run's snapshot and in-place rollback stay cheap and unchanged."""

    def __init__(self, fetch, default=None, cache=None, dirty=None, total=None):
        self.fetch, self.default, self.total_sql = fetch, default, total
        self.cache = {} if cache is None else cache
        self.dirty = {} if dirty is None else dict(dirty)

    def copy(self):
        return _Lazy(self.fetch, self.default, self.cache, self.dirty, self.total_sql)

    def _read(self, k):
        if k in self.dirty:
            return self.dirty[k]
        if k not in self.cache:
            self.cache[k] = self.fetch(k)
        return self.cache[k]

    def __contains__(self, k):
        return self._read(k) is not None

    def __getitem__(self, k):
        v = self._read(k)
        if v is None:
            raise KeyError(k)
        return v

    def get(self, k, default=None):
        v = self._read(k)
        return default if v is None else v

    def __setitem__(self, k, v):
        self.dirty[k] = v

    def values(self):
        """Only `ledger.total` iterates balances: the SQL sum, corrected for this call's writes."""
        total = self.total_sql()
        for k, v in self.dirty.items():
            total += v - (self.cache[k] if k in self.cache else self.fetch(k) or 0)
        return [total]

    def __iter__(self):
        raise TypeError("a lazy store cannot be iterated (that would load the whole world)")


class _LazyMaps(dict):
    """state name -> _Lazy, for every declared state (so `maps.get(x, {})` never misses)."""

    def copy_maps(self):
        out = _LazyMaps()
        for x, m in self.items():
            out[x] = m.copy()
        return out


class LazyWorld(World):
    def copy(self):
        return LazyWorld(self.bal.copy(), self.own.copy(), self.clock, list(self.audit), self.maps.copy_maps(),
                         {c: list(e) for c, e in self.out.items()})

    def written_keys(self):
        """{"Account": keys, "Int": keys} this call wrote, for the key-scoped invariant check."""
        keys = {"Account": set(self.bal.dirty), "Int": set()}
        for x, m in self.maps.items():
            keys[m.key_type].update(m.dirty)
        return keys


def _tuplify(x):
    """JSON gives lists back; events and constructor values are tuples in the interpreter."""
    return tuple(_tuplify(y) for y in x) if isinstance(x, list) else x


def event_text(ev):
    if isinstance(ev, tuple):
        return " ".join([ev[0]] + [f"({event_text(a)})" if isinstance(a, tuple) else str(a) for a in ev[1:]])
    return str(ev)


def console_sink(seq, ev):
    print(f"audit #{seq}: {event_text(ev)}", flush=True)


def stdout_sink(seq, ev):
    """The `console` channel: one line per event, the event's text without the constructor."""
    print(" ".join(event_text(a) if isinstance(a, tuple) else str(a) for a in ev[1:]) if isinstance(ev, tuple)
          else str(ev), flush=True)


def _ordered_events(world, defs):
    """A call's events as (channel, event): audit first, then channels in declaration order. The
    order across channels is a host convention; specs fix only the order within one channel."""
    return [("audit", ev) for ev in world.audit] + \
        [(c, ev) for c in getattr(defs, "channels", []) for ev in world.out.get(c, [])]


class Host:
    def __init__(self, defs, db=":memory:", clock=None, sinks=None, gate=True, channel_sinks=None,
                 net=None, reports=None, endpoints=None, providers=None):
        self.defs = defs
        # assumed handlers (observations inside a call): a provider per handler, checked against the
        # assumed spec on every call by the interpreter, and every value recorded for replay
        self.providers = dict(DEFAULT_PROVIDERS if providers is None else providers)
        missing = sorted(set(getattr(defs, "handlers", {})) - set(self.providers))
        if missing:
            raise ConfigError("the module declares " + ", ".join(missing) + ", which this host does not provide")
        self.interp = Interp(defs, provider=lambda name, args: self.providers[name](args))
        self.clock = clock or (lambda: int(time.time()))
        self.sinks = [console_sink] if sinks is None else sinks             # the audit channel
        self.channel_sinks = {"console": [stdout_sink]} if channel_sinks is None else channel_sinks
        self.db = sqlite3.connect(db, isolation_level=None, check_same_thread=False)   # explicit transactions
        self.lock = threading.RLock()          # one top-level call at a time, as the proofs assume
        self.db.executescript(SCHEMA)
        if "observed" not in [r[1] for r in self.db.execute("pragma table_info(log)")]:
            self.db.execute("alter table log add column observed text not null default '[]'")
        cols = [r[1] for r in self.db.execute("pragma table_info(outbox)")]
        if "channel" not in cols:              # a database from before channels
            self.db.execute("alter table outbox add column channel text not null default 'audit'")
        # Verification licenses running: only clean definitions are served (gate=False serves all).
        self.reports = reports or analyse(defs, mutation=False)
        self.gate = gate
        # The network is policy: every request the module can send needs a configured endpoint,
        # checked now, at startup (endpoints=None with net requests in the module is refused too).
        reqs = net_requests(defs)
        if reqs and endpoints is None:
            raise ConfigError("the module sends " + ", ".join(sorted(reqs)) + " on net, and there is no host config")
        self.endpoints = endpoints or {}
        self.net = net                         # a sink returning the reply calls; None = HttpNet
        if reqs and self.net is None:
            self.net = HttpNet(self.endpoints)
        self.inbox = []                        # replies waiting to become top-level calls

    # --- the world in SQLite
    def init(self, world_src):
        """Create the genesis world once. Returns False if the database already has one."""
        if self.db.execute("select 1 from meta where k='genesis'").fetchone():
            return False
        rest, maps = split_maps(world_src)
        w = World.from_example(parse_world(rest, None), 0, maps)
        broken = self.interp.broken_invariants(w)
        if broken:
            raise ValueError(f"genesis world breaks invariant {broken[0][0]} at {broken[0][1]}")
        self.db.execute("begin immediate")
        try:
            self.db.execute("insert into meta values('genesis', ?)", (json.dumps(self._dump(w)),))
            self._write(World({}, {}, 0, [], {}), w)
            self.db.execute("commit")
        except BaseException:
            self.db.execute("rollback")
            raise
        return True

    @staticmethod
    def _dump(w):
        return {"bal": w.bal, "own": w.own,
                "maps": {s: [[k, v] for k, v in m.items()] for s, m in w.maps.items()}}

    @staticmethod
    def _undump(d):
        return World(dict(d["bal"]), dict(d["own"]), 0, [], {s: {k: v for k, v in kv} for s, kv in d["maps"].items()})

    def lazy(self):
        """The world for one call, read on demand inside the call's transaction."""
        q = self.db.execute
        one = lambda sql, *a: (q(sql, a).fetchone() or (None,))[0]
        bal = _Lazy(lambda a: one("select balance from account where name=?", a),
                    total=lambda: one("select coalesce(sum(balance), 0) from account"))
        own = _Lazy(lambda a: one("select owner from account where name=?", a))
        maps = _LazyMaps()
        for x, key_type in getattr(self.defs, "states", {}).items():
            m = _Lazy(lambda k, x=x: one("select value from cell where state=? and key=?", x, json.dumps(k)) or 0)
            m.key_type = key_type
            maps[x] = m
        return LazyWorld(bal, own, 0, [], maps)

    def _write_dirty(self, world):
        """Write exactly what the call changed (accounts are never created by a call)."""
        for a, b in world.bal.dirty.items():
            self.db.execute("update account set balance=? where name=?", (b, a))
        for x, m in world.maps.items():
            for k, v in m.dirty.items():
                self.db.execute("insert into cell values(?,?,?) on conflict(state, key) do update set value=?",
                                (x, json.dumps(k), v, v))

    def load(self):
        """The whole world (replay, `GET /state`): never on the call path."""
        bal, own, maps = {}, {}, {}
        for name, b, o in self.db.execute("select name, balance, owner from account"):
            bal[name], own[name] = b, o
        for s, k, v in self.db.execute("select state, key, value from cell"):
            maps.setdefault(s, {})[json.loads(k)] = v
        return World(bal, own, 0, [], maps)

    def _write(self, before, after):
        """Write only what the call changed (balances and map cells; maps never drop keys)."""
        for a, b in after.bal.items():
            if before.bal.get(a) != b or a not in before.own:
                self.db.execute("insert into account values(?,?,?) on conflict(name) do update set balance=?",
                                (a, b, after.own[a], b))
        for s, m in after.maps.items():
            old = before.maps.get(s, {})
            for k, v in m.items():
                if old.get(k) != v:
                    self.db.execute("insert into cell values(?,?,?) on conflict(state, key) do update set value=?",
                                    (s, json.dumps(k), v, v))

    # --- one top-level call: the only way anything changes
    def call(self, name, args):
        """Run `name args` atomically. Returns {"status": ok|refused|error, ...}."""
        with self.lock:
            return self._call(name, args)

    def _call(self, name, args):
        if name not in self.defs:
            raise KeyError(name)
        if self.gate and not self.reports[name]["clean"]:
            raise Unproven(name)
        now = self.clock()                               # the observation, read once per call
        self.interp.observations = []
        self.db.execute("begin immediate")               # serialises calls, also across processes
        try:
            world = self.lazy()
            world.clock = now
            try:
                outcome, world = self.interp.run(name, list(args), world)
            except KernError as e:
                res = {"status": "error", "error": str(e)}
            else:
                if outcome == ("ok",):
                    # tier ○, checked at runtime: at the keys this call wrote (sound: wf.invariant_problems)
                    broken = self.interp.broken_invariants(world, world.written_keys())
                    if broken:
                        res = {"status": "error", "fault": "module", "error": f"invariant broken: {broken[0][0]} at {broken[0][1]}"}
                    else:
                        res = {"status": "ok", "events": world.audit}
                        if any(world.out.values()):
                            res["out"] = {c: e for c, e in world.out.items() if e}
                        if self.defs[name].ret:
                            res["result"] = getattr(self.interp, "last_result", None)
                else:
                    res = {"status": "refused", "refused": outcome[0], "args": list(outcome[1])}
            seq = self.db.execute("insert into log(name, args, now, outcome, observed) values(?,?,?,?,?)",
                                  (name, json.dumps(list(args)), now, json.dumps(self._logged(res)),
                                   json.dumps(self.interp.observations))).lastrowid
            if res["status"] == "ok":
                self._write_dirty(world)
                for ch, ev in _ordered_events(world, self.defs):
                    self.db.execute("insert into outbox(call, event, channel) values(?,?,?)", (seq, json.dumps(ev), ch))
            self.db.execute("commit")
        except BaseException:
            self.db.execute("rollback")
            raise
        res["call"] = seq
        return res

    @staticmethod
    def _logged(res):
        return {k: res[k] for k in ("status", "refused", "args", "error") if k in res}

    def flush(self):
        """Deliver committed outbox events to every sink, then mark them. At-least-once."""
        with self.lock:
            return self._flush()

    def _flush(self):
        n = 0
        rows = self.db.execute("select seq, event, channel from outbox where delivered=0 order by seq").fetchall()
        for seq, ev, ch in rows:
            ev = _tuplify(json.loads(ev))
            if ch == "net":                    # a request; whatever comes back is a later call
                self.inbox += self.net(seq, ev)
            for sink in (self.sinks if ch == "audit" else self.channel_sinks.get(ch, [])):
                sink(seq, ev)
            self.db.execute("update outbox set delivered=1 where seq=?", (seq,))
            n += 1
        return n

    def pump(self, extra=()):
        """Run replies as new top-level calls (each one atomic and logged like any other call),
        then deliver what they emitted. Returns the replies' results."""
        with self.lock:
            self.inbox += list(extra)
            results = []
            while self.inbox:
                name, args = self.inbox.pop(0)
                results.append((name, args, self.call(name, args)))
                self._flush()
            return results

    # --- determinism
    def replay(self):
        """Re-run the call log from genesis with the recorded `now` values. Returns a list of
        mismatches (empty = the database is exactly what the log implies)."""
        g = self.db.execute("select v from meta where k='genesis'").fetchone()
        if not g:
            return ["no genesis world"]
        queue = []                                 # the recorded observations, in call order

        def replayed(n, a):
            if not queue or queue[0][0] != n or queue[0][1] != a:
                raise RuntimeError(f"replay: {n} {a} was not the next recorded observation")
            return queue.pop(0)[2]
        w, interp, bad, events = self._undump(json.loads(g[0])), Interp(self.defs, provider=replayed), [], []
        for seq, name, args, now, logged, observed in self.db.execute(
                "select seq, name, args, now, outcome, observed from log order by seq").fetchall():
            logged = json.loads(logged)
            queue[:] = json.loads(observed)
            if logged["status"] == "error":
                continue
            snap = w.copy()
            w.clock, w.audit, w.out = now, [], {}
            outcome, w = interp.run(name, _tuplify(json.loads(args)), w)
            got = "ok" if outcome == ("ok",) else outcome[0]
            want = "ok" if logged["status"] == "ok" else logged["refused"]
            if got != want:
                bad.append(f"call {seq} {name}: logged {want}, replay gives {got}")
                w = snap
                continue
            events += [[ch, json.dumps(ev)] for ch, ev in _ordered_events(w, self.defs)]
        now = self.load()
        if (now.bal, now.maps) != (w.bal, {s: m for s, m in w.maps.items() if m}):
            bad.append(f"state differs: db {now.bal} {now.maps}, replay {w.bal} {w.maps}")
        stored = [[c, e] for c, e in self.db.execute("select channel, event from outbox order by seq")]
        if stored != events:
            bad.append(f"outbox differs: {len(stored)} stored, {len(events)} from replay")
        return bad

    # --- the permission manifest: inferred, never declared
    def manifest(self):
        out = {}
        for n, r in self.reports.items():
            d = self.defs[n]
            gs = r["guarantees"]
            out[n] = {"params": [f"{p}:{t}" for p, t in d.params], "uses": sorted(uses(self.defs, d)),
                      "fails": sorted(fails(self.defs, d)), "served": r["clean"] or not self.gate,
                      "proven": f"{sum(g['status'] == 'proven' for g in gs)}/{len(gs)}"}
        return out


def _system_below(args):
    import secrets
    return secrets.randbelow(args[0])


DEFAULT_PROVIDERS = {"rand.below": _system_below}   # assumed handlers this host can provide

HOST_EFFECTS = {   # how this host realises each effect the module can name
    "ledger": "SQLite table `account`, one transaction per call",
    "clock": "wall clock, seconds; read once per call, recorded in `log`",
    "audit": "SQLite table `outbox`; delivered to stdout after commit (at-least-once)",
    "rand": "assumed handler rand.below: the OS random source (secrets); every result checked against its "
            "assumed spec and recorded in `log.observed` for replay",
    "console": "SQLite table `outbox` (channel console); one line per event on stdout after commit (at-least-once)",
    "net": "SQLite table `outbox` (channel net); each event is an HTTP request from the host config, sent "
           "after commit (at-least-once); the reply or failure arrives as a new top-level call",
}


def manifest_text(host):
    m = host.manifest()
    allu = sorted({u for v in m.values() for u in v["uses"]})
    lines = ["module may use: " + ", ".join(allu), "host provides:"]
    for root in sorted({u.split(".")[0] for u in allu}):
        lines.append(f"  {root:<8} {HOST_EFFECTS.get(root, 'SQLite table `cell`, one transaction per call')}")
    for ctor, ep in sorted(host.endpoints.items()):
        pattern = ep.url
        for i in range(1, 10):
            pattern = pattern.replace("{%d}" % i, "*")
        lines.append(f"  net: {ctor} -> {ep.method} {pattern}  reply {ep.reply}, fail {ep.fail}")
    lines.append("nothing else: the module cannot name any other effect or endpoint.")
    lines.append("")
    for n, v in m.items():
        mark = "serve" if v["served"] else "BLOCK"
        lines.append(f"{mark} {n} {' '.join(v['params'])}  proven {v['proven']}  "
                     f"uses {', '.join(v['uses']) or '-'}  fails {', '.join(v['fails']) or '-'}")
    return "\n".join(lines)


ASSUMPTIONS = [
    "Calls run one at a time (a lock in this process; `begin immediate` across processes). The proofs "
    "assume serial top-level calls.",
    "Every event is delivered at least once, after its call commits. A crash between delivering and marking "
    "an event sends it again (docs/HOST.md has the measured count). Consumers dedupe on `seq`.",
    "The order of events across channels is audit first, then channels in declaration order. Specs fix the "
    "order within a channel only.",
    "`clock.now` is the wall clock in seconds, read once per call and recorded for replay.",
]
NET_ASSUMPTIONS = [
    "A network reply becomes a new top-level call. The host decodes each `$.field` as an Int; a timeout, "
    "a non-2xx status or anything that isn't an Int becomes the `fail` call. Kern never sees bytes.",
    "There is no request timeout: a reply that never arrives, or one the module refuses, leaves the "
    "request pending (docs/HOST.md has the measured count).",
]


def host_facts(host):
    """The data for render.host_html: effects the module uses, endpoints, assumptions."""
    used = sorted({u.split(".")[0] for v in host.manifest().values() for u in v["uses"]})
    effects = {u: HOST_EFFECTS.get(u, "SQLite table `cell`, one transaction per call") for u in used}
    eps = [(c, ep.method, ep.url, ep.reply, ep.fail) for c, ep in sorted(host.endpoints.items())]
    asm = ASSUMPTIONS + (NET_ASSUMPTIONS if host.endpoints else [])
    return {"effects": effects, "endpoints": eps, "assumptions": asm}


# --- the console: one call per line on stdin, outbox on stdout
def repl(host, lines=None, out=print):
    for line in (lines if lines is not None else sys.stdin):
        line = line.strip()
        if not line or line.startswith("#"):
            continue
        if line == "state":
            w = host.load()
            out(json.dumps({"balances": w.bal, "state": {s: {str(k): v for k, v in m.items()} for s, m in w.maps.items()}}))
            continue
        name, *args = line.split()
        args = [int(x) if x.lstrip("-").isdigit() else x for x in args]
        try:
            r = host.call(name, args)
        except KeyError:
            out(f"error: no definition named {name}")
            continue
        except Unproven:
            out(f"error: {name} is not proven clean; this host does not run it")
            continue
        if r["status"] == "ok":
            out("ok" + (f" {r['result']}" if "result" in r else ""))
        elif r["status"] == "refused":
            out(" ".join(["refused", r["refused"], *map(str, r["args"])]))
        else:
            out(f"error: {r['error']}")
        host.flush()


# --- the network: POST /NAME with a JSON object of named parameters
STATUS = {"ok": 200, "refused": 409, "error": 400}


def make_handler(host):
    class H(BaseHTTPRequestHandler):
        def _send(self, code, body):
            data = json.dumps(body).encode()
            self.send_response(code)
            self.send_header("content-type", "application/json")
            self.send_header("content-length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)

        def log_message(self, *a):
            pass

        def do_GET(self):
            if self.path == "/manifest":
                return self._send(200, host.manifest())
            if self.path == "/state":
                w = host.load()
                return self._send(200, {"balances": w.bal,
                                        "state": {s: {str(k): v for k, v in m.items()} for s, m in w.maps.items()}})
            self._send(404, {"error": "GET /manifest or /state; POST /NAME"})

        def do_POST(self):
            name = self.path.strip("/")
            if name not in host.defs:
                return self._send(404, {"error": f"no definition named {name!r}"})
            d = host.defs[name]
            try:
                body = json.loads(self.rfile.read(int(self.headers.get("content-length") or 0)) or b"{}")
            except ValueError:
                return self._send(400, {"error": "body is not JSON"})
            want = [p for p, _ in d.params]
            if not isinstance(body, dict) or sorted(body) != sorted(want):
                return self._send(400, {"error": f"want a JSON object with exactly {want}"})
            try:
                r = host.call(name, [body[p] for p in want])     # types are checked by Interp.run
            except Unproven:
                return self._send(403, {"error": f"{name} is not proven clean; this host does not run it"})
            except Exception as e:                                # a host fault; the call rolled back
                return self._send(500, {"error": f"host: {type(e).__name__}: {e}"})
            self._send(500 if r.get("fault") == "module" else STATUS[r["status"]], r)   # ○ tier fired
            host.flush()                                          # after commit, after the reply
    return H


def serve(host, port=8080, ready=None):
    srv = HTTPServer(("127.0.0.1", port), make_handler(host))    # single-threaded: calls are serial
    if ready:
        ready(srv)
    else:
        print(f"serving on http://127.0.0.1:{srv.server_address[1]}", flush=True)
    srv.serve_forever()


def main(argv=None):
    ap = argparse.ArgumentParser(prog="python -m kern.host")
    ap.add_argument("cmd", choices=["serve", "repl", "replay", "manifest", "page"])
    ap.add_argument("--html", help="page: where to write the proof page with the host section")
    ap.add_argument("file")
    ap.add_argument("--db", default=":memory:")
    ap.add_argument("--world")
    ap.add_argument("--port", type=int, default=8080)
    ap.add_argument("--no-gate", action="store_true", help="also serve definitions that are not proven clean")
    ap.add_argument("--config", help="host config (endpoints); default FILE.host next to the module")
    a = ap.parse_args(argv)
    try:
        defs = check_module(parse_module(open(a.file).read()))
    except ParseError as e:
        print(f"{a.file}: {e}", file=sys.stderr)
        return 2
    import os
    cfg = a.config or os.path.splitext(a.file)[0] + ".host"
    try:
        endpoints = load_config(open(cfg).read(), defs) if os.path.exists(cfg) else None
        host = Host(defs, a.db, gate=not a.no_gate, endpoints=endpoints)
    except ConfigError as e:
        print(f"{a.file}: host refuses to start: {e}", file=sys.stderr)
        return 2
    if a.world and not host.init(a.world):
        print("note: database already has a world; --world ignored", file=sys.stderr)
    if a.cmd == "manifest":
        print(manifest_text(host))
    elif a.cmd == "page":
        from . import render, why
        whys = {n: why.status({}, d, defs) for n, d in defs.items()}
        reports = analyse(defs)
        with open(a.html or "proof.html", "w") as f:
            f.write(render.page(reports, whys, a.file, host=host_facts(host)))
        print(f"proof page with host section: {a.html or 'proof.html'}")
    elif a.cmd == "replay":
        bad = host.replay()
        n = host.db.execute("select count(*) from log").fetchone()[0]
        print("\n".join(bad) if bad else f"replay of {n} logged calls matches the database")
        return 1 if bad else 0
    elif a.cmd == "repl":
        host.flush()                         # anything left undelivered by a crash goes out first
        repl(host)
    else:
        host.flush()
        try:
            serve(host, a.port)
        except KeyboardInterrupt:
            pass
    return 0


if __name__ == "__main__":
    sys.exit(main())

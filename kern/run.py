"""Concrete interpreter. All effects go through handlers; a top-level call is atomic
(failure rolls the world back), so `keep` and `ensure` only have to hold on success."""
from __future__ import annotations

import copy
from dataclasses import dataclass, field

from .syntax import code, emit_code


@dataclass
class World:
    bal: dict                      # account -> int
    own: dict                      # account -> owner
    clock: int = 0
    audit: list = field(default_factory=list)
    maps: dict = field(default_factory=dict)      # declared state -> {key: Int}; missing keys are 0
    out: dict = field(default_factory=dict)       # declared channel -> events (`audit` stays in .audit)

    def copy(self):
        """A full copy: balances and owners are ints/strings, audit events are tuples."""
        return World(dict(self.bal), dict(self.own), self.clock, list(self.audit),
                     {k: dict(v) for k, v in self.maps.items()}, {c: list(e) for c, e in self.out.items()})

    def events(self, channel):
        return self.audit if channel == "audit" else self.out.get(channel, [])

    @classmethod
    def from_example(cls, w, now=0, maps=None):
        return cls({a: b for a, (b, _) in w.items()}, {a: o for a, (_, o) in w.items()}, now, [],
                   {k: dict(v) for k, v in (maps or {}).items()})


class Fail(Exception):
    def __init__(self, ctor, args):
        super().__init__(ctor)
        self.ctor, self.args_ = ctor, args


class KernError(Exception):
    pass


def _acct(w, a):
    if a not in w.bal:
        raise KernError(f"unknown account {a!r}")
    return a


def _debit(w, a, n):
    w.bal[_acct(w, a)] -= n


def _credit(w, a, n):
    w.bal[_acct(w, a)] += n


def _now(w):
    """The time of the current top-level call: the same value for every read inside it."""
    return w.clock


# op -> (arity, effect label, implementation(world, *args))
HANDLERS = {
    "ledger.balance": (1, "ledger.read", lambda w, a: w.bal[_acct(w, a)]),
    "ledger.owner": (1, "ledger.read", lambda w, a: w.own[_acct(w, a)]),
    "ledger.total": (0, "ledger.read", lambda w: sum(w.bal.values())),
    "ledger.debit": (2, "ledger.write", _debit),
    "ledger.credit": (2, "ledger.write", _credit),
    "clock.now": (0, "clock", _now),
    "audit.emit": (1, "audit", lambda w, ev: w.audit.append(ev)),
}


def state_op(defs, name):
    """`X.get` / `X.set` for a module's `state X Key` declaration: (X, op, key type), else None."""
    states = getattr(defs, "states", {})
    x, _, op = name.rpartition(".")
    if x in states and op in ("get", "set"):
        return x, op, states[x]
    return None


def channel_op(defs, name):
    """`C.emit` for a module's `channel C` declaration: C, else None."""
    c, _, op = name.rpartition(".")
    return c if op == "emit" and c in getattr(defs, "channels", []) else None


def is_handler(defs, name):
    return name in HANDLERS or state_op(defs, name) is not None or channel_op(defs, name) is not None


def effect_of(defs, name):
    if name in HANDLERS:
        return HANDLERS[name][1]
    if channel_op(defs, name):
        return channel_op(defs, name)
    x, op, _ = state_op(defs, name)
    return f"{x}.{'write' if op == 'set' else 'read'}"


def emit_channels(d):
    """The channels d's spec says something about, `audit` first; a channel with no clause is
    unconstrained (as the audit log is for a definition with no `emit` at all)."""
    chans = [d.emit_channel.get(i, "audit") for i in range(len(d.emits))]
    return sorted(set(chans), key=lambda c: (c != "audit", c))


def _restore(world, snap):
    """Roll back in place, so every reference the caller holds sees the rollback."""
    world.__dict__.clear()
    world.__dict__.update(snap.__dict__)


class Interp:
    def __init__(self, defs):
        self.defs = defs

    # --- expressions; `pre` is the world unprimed reads see, `post` the one primed reads see
    def ev(self, e, env, pre, post=None):
        k = e[0]
        if k == "int":
            return e[1]
        if k == "all":                          # every j in [A, B)
            lo, hi = self.ev(e[2], env, pre, post), self.ev(e[3], env, pre, post)
            return all(self.ev(e[4], dict(env, **{e[1]: v}), pre, post) for v in range(lo, hi))
        if k == "bin":
            op = e[1]
            if op == "and":
                return self.ev(e[2], env, pre, post) and self.ev(e[3], env, pre, post)
            if op == "or":
                return self.ev(e[2], env, pre, post) or self.ev(e[3], env, pre, post)
            if op == "when":                    # `E when C`: C reads the before-state
                return (not self.ev(e[3], env, pre)) or bool(self.ev(e[2], env, pre, post))
            l, r = self.ev(e[2], env, pre, post), self.ev(e[3], env, pre, post)
            return {"+": lambda: l + r, "-": lambda: l - r, "*": lambda: l * r, "/": lambda: l // r, "=": lambda: l == r, "!=": lambda: l != r,
                    ">=": lambda: l >= r, "<=": lambda: l <= r, ">": lambda: l > r, "<": lambda: l < r}[op]()
        if k == "name":
            name, primed = e[1], e[2]
            if name in env:
                return env[name]
            return self.call(name, [], pre if not primed else post, primed)
        if k == "app":
            (name, primed), args = e[1], e[2]
            vals = [self.ev(a, env, pre, post) for a in args]
            return self.call(name, vals, pre if not primed else post, primed)
        raise KernError(f"cannot evaluate {e}")

    def call(self, name, args, world, primed=False):
        if primed and world is None:
            raise KernError(f"{name}' (after-state) only allowed in ensure/keep")
        if name in HANDLERS:
            arity, _, fn = HANDLERS[name]
            if len(args) != arity:
                raise KernError(f"{name} takes {arity} argument(s), got {len(args)}")
            return fn(world, *args)
        ch = channel_op(self.defs, name)
        if ch:
            world.out.setdefault(ch, []).append(args[0])
            return None
        st = state_op(self.defs, name)
        if st:
            x, op, key_type = st
            if key_type == "Account":
                _acct(world, args[0])
            if op == "get":
                return world.maps.get(x, {}).get(args[0], 0)
            world.maps.setdefault(x, {})[args[0]] = args[1]
            return None
        if name in self.defs:
            return self.call_def(self.defs[name], args, world)
        if name[:1].isupper():
            return (name, *args)            # constructor value
        if name == "distinct":
            return len(set(args)) == len(args)
        raise KernError(f"unknown name {name!r}")

    def call_def(self, d, args, world):
        if len(args) != len(d.params):
            raise KernError(f"{d.name} takes {len(d.params)} argument(s), got {len(args)}")
        env = {p: a for (p, _), a in zip(d.params, args)}
        self.block(d.needs + d.body, env, world)
        return env.get("result")                 # functions (-> Int) set it; others return nothing

    def block(self, stmts, env, world):
        for s in stmts:
            if s[0] == "need":
                if not self.ev(s[1], env, world):
                    raise Fail(s[2], [self.ev(a, env, world) for a in s[3]])
            elif s[0] == "let":
                env[s[1]] = self.ev(s[2], env, world)
            elif s[0] == "if":
                self.block(s[2] if self.ev(s[1], env, world) else s[3], env, world)
            elif s[0] == "for":                 # bounds evaluated once; the variable is never assigned
                lo, hi = self.ev(s[2], env, world), self.ev(s[3], env, world)
                for v in range(lo, hi):
                    env[s[1]] = v
                    self.block(s[5], env, world)
            else:
                self.ev(s[1], env, world)

    def run(self, name, args, world):
        """Atomic top-level call. Returns (outcome, world_after).

        Arguments are checked at the boundary, before anything runs: with the type checker,
        accounts inside a body only ever come from parameters, so an unknown account can only
        enter here. That keeps every outcome either ok or a declared (inferred) failure."""
        d = self.defs[name]
        if len(args) != len(d.params):
            raise KernError(f"bad call: {name} takes {len(d.params)} argument(s), got {len(args)}")
        for (p, t), a in zip(d.params, args):
            if t == "Account" and (not isinstance(a, str) or a not in world.bal):
                raise KernError(f"bad call: account {a!r} for `{p}` is not in the world")
            if t == "Int" and (not isinstance(a, int) or isinstance(a, bool)):
                raise KernError(f"bad call: `{p}` must be an Int, got {a!r}")
        snap = world.copy()
        try:
            self.last_result = self.call_def(self.defs[name], args, world)
            outcome = ("ok",)
        except Fail as f:
            _restore(world, snap)
            outcome = (f.ctor, f.args_)
        except BaseException:          # runtime errors are atomic too: nothing the call did survives
            _restore(world, snap)      # (and no time passes: the call did not happen)
            raise
        world.clock += 1               # time passes between calls, refused ones included
        return outcome, world

    def holds(self, d, args, pre, post):
        """Which keep/ensure/emit clauses fail for this (pre, post) pair. Returns [(kind, clause source)]."""
        env = {p: a for (p, _), a in zip(d.params, args)}
        if d.ret:
            env["result"] = getattr(self, "last_result", None)
        bad = []
        for kind, clauses in (("keep", d.keeps), ("ensure", d.ensures)):
            for c in clauses:
                if kind == "keep":
                    ok = self.ev(c, env, pre) == self.ev(c, env, post)
                else:
                    ok = bool(self.ev(c, env, pre, post))
                if not ok:
                    bad.append((kind, code(c)))
        for ch in emit_channels(d):
            if not self.emitted_ok(d, env, pre, post, ch):
                bad.append(("emit", "; ".join(emit_code(ev) for i, ev in enumerate(d.emits)
                                              if d.emit_channel.get(i, "audit") == ch)))
        return bad

    def unstated_refusal(self, d, args, before, outcome):
        """`ok otherwise` is broken by this run: every spec `need` held, yet the call refused."""
        if not d.complete or outcome == ("ok",):
            return False
        env = {p: a for (p, _), a in zip(d.params, args)}
        w = before.copy()
        return all(self.ev(n[1], env, w) for n in d.needs)

    def broken_invariants(self, world, keys=None):
        """Module invariants that some key of this world violates: [(text, key)].

        keys = {"Account": {...}, "Int": {...}} checks only those keys (the host passes the keys a call
        wrote). That is sound because an invariant may only read state at its own bound key
        (wf.invariant_problems), so an unwritten key keeps whatever the invariant said about it."""
        out = []
        for var, t, cond in getattr(self.defs, "invariants", []):
            if keys is not None:
                keys_t = sorted(keys.get(t, ()), key=str)
            else:
                keys_t = list(world.bal) if t == "Account" else sorted({k for m in world.maps.values() for k in m
                                                                         if isinstance(k, int)})
            for k in keys_t:
                if not self.ev(cond, {var: k}, world):
                    out.append((f"{var}:{t} {code(cond)}", k))
                    break
        return out

    def emitted_ok(self, d, env, pre, post, channel="audit"):
        """The call appended exactly the events its `emit` clauses list for this channel, in order;
        `_` matches anything."""
        before, after = pre.events(channel), post.events(channel)
        got = after[len(before):]
        want = [ev for i, ev in enumerate(d.emits) if d.emit_channel.get(i, "audit") == channel
                and (len(ev) < 3 or self.ev(ev[2], env, pre.copy()))]
        if after[:len(before)] != before or len(got) != len(want):
            return False
        for (ctor, args, *_), ev in zip(want, got):
            if not isinstance(ev, tuple) or ev[0] != ctor or len(ev) != len(args) + 1:
                return False
            for a, v in zip(args, ev[1:]):
                if a != ("name", "_", False) and self.ev(a, env, pre.copy()) != v:
                    return False
        return True

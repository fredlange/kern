"""Concrete interpreter. All effects go through handlers; a top-level call is atomic
(failure rolls the world back), so `keep` and `ensure` only have to hold on success."""
from __future__ import annotations

import copy
from dataclasses import dataclass, field


@dataclass
class World:
    bal: dict                      # account -> int
    own: dict                      # account -> owner
    clock: int = 0
    audit: list = field(default_factory=list)

    @classmethod
    def from_example(cls, w):
        return cls({a: b for a, (b, _) in w.items()}, {a: o for a, (_, o) in w.items()})


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
    w.clock += 1
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


class Interp:
    def __init__(self, defs):
        self.defs = defs

    # --- expressions; `pre` is the world unprimed reads see, `post` the one primed reads see
    def ev(self, e, env, pre, post=None):
        k = e[0]
        if k == "int":
            return e[1]
        if k == "bin":
            op = e[1]
            if op == "and":
                return self.ev(e[2], env, pre, post) and self.ev(e[3], env, pre, post)
            if op == "or":
                return self.ev(e[2], env, pre, post) or self.ev(e[3], env, pre, post)
            l, r = self.ev(e[2], env, pre, post), self.ev(e[3], env, pre, post)
            return {"+": lambda: l + r, "-": lambda: l - r, "=": lambda: l == r, "!=": lambda: l != r,
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
        if name in self.defs:
            return self.call_def(self.defs[name], args, world)
        if name[:1].isupper():
            return (name, *args)            # constructor value
        raise KernError(f"unknown name {name!r}")

    def call_def(self, d, args, world):
        if len(args) != len(d.params):
            raise KernError(f"{d.name} takes {len(d.params)} argument(s), got {len(args)}")
        env = {p: a for (p, _), a in zip(d.params, args)}
        for s in d.needs + d.body:
            if s[0] == "need":
                if not self.ev(s[1], env, world):
                    raise Fail(s[2], [self.ev(a, env, world) for a in s[3]])
            elif s[0] == "let":
                env[s[1]] = self.ev(s[2], env, world)
            else:
                self.ev(s[1], env, world)
        return None

    def run(self, name, args, world):
        """Atomic top-level call. Returns (outcome, world_after)."""
        snap = copy.deepcopy(world)
        try:
            self.call_def(self.defs[name], args, world)
            return ("ok",), world
        except Fail as f:
            return (f.ctor, f.args_), snap

    def holds(self, d, args, pre, post):
        """Which keep/ensure clauses hold for this (pre, post) pair. Returns list of failing clause sources."""
        env = {p: a for (p, _), a in zip(d.params, args)}
        bad = []
        for kind, clauses in (("keep", d.keeps), ("ensure", d.ensures)):
            for c in clauses:
                if kind == "keep":
                    ok = self.ev(c, env, pre) == self.ev(c, env, post)
                else:
                    ok = bool(self.ev(c, env, pre, post))
                if not ok:
                    bad.append((kind, c))
        return bad

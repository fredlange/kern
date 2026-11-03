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

    @classmethod
    def from_example(cls, w, now=0):
        return cls({a: b for a, (b, _) in w.items()}, {a: o for a, (_, o) in w.items()}, now)


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
        snap = copy.deepcopy(world)
        try:
            self.call_def(self.defs[name], args, world)
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
        bad = []
        for kind, clauses in (("keep", d.keeps), ("ensure", d.ensures)):
            for c in clauses:
                if kind == "keep":
                    ok = self.ev(c, env, pre) == self.ev(c, env, post)
                else:
                    ok = bool(self.ev(c, env, pre, post))
                if not ok:
                    bad.append((kind, code(c)))
        if d.emits and not self.emitted_ok(d, env, pre, post):
            bad.append(("emit", "; ".join(emit_code(ev) for ev in d.emits)))
        return bad

    def unstated_refusal(self, d, args, before, outcome):
        """`ok otherwise` is broken by this run: every spec `need` held, yet the call refused."""
        if not d.complete or outcome == ("ok",):
            return False
        env = {p: a for (p, _), a in zip(d.params, args)}
        w = copy.deepcopy(before)
        return all(self.ev(n[1], env, w) for n in d.needs)

    def emitted_ok(self, d, env, pre, post):
        """The call appended exactly the events its `emit` clauses list, in order; `_` matches anything."""
        got = post.audit[len(pre.audit):]
        if post.audit[:len(pre.audit)] != pre.audit or len(got) != len(d.emits):
            return False
        for (ctor, args), ev in zip(d.emits, got):
            if not isinstance(ev, tuple) or ev[0] != ctor or len(ev) != len(args) + 1:
                return False
            for a, v in zip(args, ev[1:]):
                if a != ("name", "_", False) and self.ev(a, env, copy.deepcopy(pre)) != v:
                    return False
        return True

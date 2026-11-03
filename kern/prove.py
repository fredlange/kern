"""Symbolic execution to Z3. Calls to other definitions are verified *modularly*: the
prover uses only the callee's spec (needs, ensures, keeps), never its body — the same
"read the interface, not the code" rule the agent follows."""
from __future__ import annotations

import itertools

import z3

from .infer import uses
from .run import Interp, World
from .syntax import code

_fresh = itertools.count()


class Sym:
    def __init__(self, bal, own, total):
        self.bal, self.own, self.total = bal, own, total

    def copy(self):
        return Sym(self.bal, self.own, self.total)


def fresh_int(p):
    return z3.Int(f"{p}{next(_fresh)}")


class Prover:
    def __init__(self, defs):
        self.defs = defs
        self.relied_on = set()

    def sev(self, e, env, cur, post=None):
        k = e[0]
        if k == "int":
            return z3.IntVal(e[1])
        if k == "bin":
            l, r = self.sev(e[2], env, cur, post), self.sev(e[3], env, cur, post)
            op = e[1]
            return {"+": lambda: l + r, "-": lambda: l - r, "=": lambda: l == r, "!=": lambda: l != r,
                    ">=": lambda: l >= r, "<=": lambda: l <= r, ">": lambda: l > r, "<": lambda: l < r,
                    "and": lambda: z3.And(l, r), "or": lambda: z3.Or(l, r)}[op]()
        if k == "name":
            if e[1] in env:
                return env[e[1]]
            return self.scall(e[1], [], post if e[2] else cur)
        if k == "app":
            (name, primed), args = e[1], e[2]
            vals = [self.sev(a, env, cur, post) for a in args]
            return self.scall(name, vals, post if primed else cur)
        raise ValueError(e)

    def scall(self, name, a, S):
        if name == "ledger.balance":
            return z3.Select(S.bal, a[0])
        if name == "ledger.owner":
            return z3.Select(S.own, a[0])
        if name == "ledger.total":
            return S.total
        if name in ("ledger.debit", "ledger.credit"):
            sign = -1 if name == "ledger.debit" else 1
            S.bal = z3.Store(S.bal, a[0], z3.Select(S.bal, a[0]) + sign * a[1])
            S.total = S.total + sign * a[1]
            return None
        if name == "clock.now":
            return fresh_int("now")
        if name == "audit.emit":
            return None
        if name in self.defs:
            return self.modular_call(self.defs[name], a, S)
        if name[:1].isupper():
            return None  # constructor value: irrelevant to the proved properties
        raise ValueError(f"prover: unknown {name}")

    def modular_call(self, callee, args, S):
        self.relied_on.add(callee.name)
        env = {p: v for (p, _), v in zip(callee.params, args)}
        for n in callee.needs:                     # success path: callee's guards passed
            self.pc.append(self.sev(n[1], env, S))
        pre = S.copy()
        if "ledger.write" in uses(self.defs, callee):
            S.bal = z3.Array(f"bal{next(_fresh)}", z3.IntSort(), z3.IntSort())
            S.total = fresh_int("total")
        for c in callee.ensures:
            self.pc.append(self.sev(c, env, pre, S))
        for c in callee.keeps:
            self.pc.append(self.sev(c, env, pre) == self.sev(c, env, S))
        return None

    def prove(self, d):
        self.relied_on = set()
        pre = Sym(z3.Array("bal", z3.IntSort(), z3.IntSort()), z3.Array("own", z3.IntSort(), z3.IntSort()),
                  z3.Int("total"))
        env = {p: z3.Int(p) for p, _ in d.params}
        params = dict(env)
        self.pc, S = [], pre.copy()
        for s in d.needs + d.body:
            if s[0] == "need":
                self.pc.append(self.sev(s[1], env, S))
            elif s[0] == "let":
                env[s[1]] = self.sev(s[2], env, S)
            else:
                self.sev(s[1], env, S)
        results = []
        goals = [("keep", c, self.sev(c, env, pre) == self.sev(c, env, S)) for c in d.keeps] + \
                [("ensure", c, self.sev(c, env, pre, S)) for c in d.ensures]
        for kind, c, g in goals:
            solver = z3.Solver()
            solver.set("timeout", 5000)
            solver.add(*self.pc, z3.Not(g))
            r = solver.check()
            if r == z3.unsat:
                results.append({"kind": kind, "clause": code(c), "status": "proven"})
            elif r == z3.sat:
                nice = z3.Solver()
                nice.add(*self.pc, z3.Not(g))
                for p, t in d.params:
                    v = params[p]
                    nice.add(v >= 0, v <= 1000) if t == "Int" else nice.add(v >= 0, v <= 3)
                    if t == "Account":
                        nice.add(z3.Select(pre.bal, v) >= 0, z3.Select(pre.bal, v) <= 1000,
                                 z3.Select(pre.own, v) >= 0, z3.Select(pre.own, v) <= 1)
                m = nice.model() if nice.check() == z3.sat else solver.model()
                results.append({"kind": kind, "clause": code(c), "status": "refuted",
                                "counterexample": self.render_cex(d, m, params, pre)})
            else:
                results.append({"kind": kind, "clause": code(c), "status": "unknown"})
        return results, sorted(self.relied_on)

    def render_cex(self, d, m, params, pre):
        """Turn a Z3 model into an `eg` line, then replay it in the interpreter to confirm."""
        letters, owners, world, args = {}, {}, {}, []
        for p, t in d.params:
            v = m.eval(params[p], model_completion=True).as_long()
            if t == "Account":
                if v not in letters:
                    letters[v] = "ABCDEFGH"[len(letters)]
                    bal = m.eval(z3.Select(pre.bal, params[p]), model_completion=True).as_long()
                    o = m.eval(z3.Select(pre.own, params[p]), model_completion=True).as_long()
                    owners.setdefault(o, f"u{len(owners) + 1}")
                    world[letters[v]] = (bal, owners[o])
                args.append(letters[v])
            else:
                args.append(v)
        w = World.from_example(world)
        interp = Interp(self.defs)
        before = World(dict(w.bal), dict(w.own))
        outcome, after = interp.run(d.name, args, w)
        world_src = " ".join(f"{a}:{b}@{o}" for a, (b, o) in world.items())
        after_src = " ".join(f"{a}:{after.bal[a]}" for a in world)
        out_src = "ok" if outcome == ("ok",) else " ".join([outcome[0], *map(str, outcome[1])])
        violated = interp.holds(d, args, before, after) if outcome == ("ok",) else []
        return {"eg": f"eg {world_src} | {d.name} {' '.join(map(str, args))} -> {out_src} | {after_src}",
                "confirmed_by_interpreter": bool(violated)}

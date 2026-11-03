"""Symbolic execution to Z3. Calls to other definitions are verified *modularly*: the
prover uses only the callee's spec (needs, ensures, keeps), never its body — the same
"read the interface, not the code" rule the agent follows."""
from __future__ import annotations

import copy
import itertools

import z3

from .infer import ANY_ACCOUNT, callees, fails, lifted, uses, writes
from .run import Interp, World
from .syntax import code, emit_code

_fresh = itertools.count()


class Sym:
    """Symbolic state. `audit` holds only the events appended during the call being proved
    (the handler can only append): ("ev", Ctor, [z3 args]) or ("opaque", callee) when a callee
    emits events its spec does not describe."""

    def __init__(self, bal, own, total, audit=(), now=None):
        self.bal, self.own, self.total, self.audit = bal, own, total, list(audit)
        self.now = now if now is not None else z3.Int("now")   # the call's time; never changes

    def copy(self):
        return Sym(self.bal, self.own, self.total, self.audit, self.now)


WILD = ("name", "_", False)


def _ev_eq(x, y):
    """Z3 condition for two symbolic values (ints or constructor values) being equal."""
    if isinstance(x, tuple) or isinstance(y, tuple):
        if not (isinstance(x, tuple) and isinstance(y, tuple)) or x[1] != y[1] or len(x[2]) != len(y[2]):
            return z3.BoolVal(False)
        return z3.And(*[z3.BoolVal(True)] + [_ev_eq(a, b) for a, b in zip(x[2], y[2])])
    return x == y


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
            return S.now
        if name == "audit.emit":
            S.audit = S.audit + [a[0] if isinstance(a[0], tuple) else ("opaque", "audit.emit")]
            return None
        if name in self.defs:
            return self.modular_call(self.defs[name], a, S)
        if name[:1].isupper():
            return ("ev", name, a)          # constructor value
        raise ValueError(f"prover: unknown {name}")

    def modular_call(self, callee, args, S):
        self.relied_on.add(callee.name)
        env = {p: v for (p, _), v in zip(callee.params, args)}
        for n in callee.needs:                     # success path: callee's guards passed
            self.refusal_point(self.sev(n[1], env, S))
        if not callee.complete and self.refuses_beyond_spec(callee):
            self.opaque_refusers.append(callee.name)   # it may refuse for reasons its spec omits
        for cond, *_ in lifted(self.defs, callee):  # ... and its inferred body-level guards
            self.pc.append(self.sev(cond, env, S))
        pre = S.copy()
        written = writes(self.defs, callee)       # inferred frame: every other balance is unchanged
        if ANY_ACCOUNT in written:
            S.bal = z3.Array(f"bal{next(_fresh)}", z3.IntSort(), z3.IntSort())
        for p in sorted(written - {ANY_ACCOUNT}):
            S.bal = z3.Store(S.bal, env[p], fresh_int(f"bal_{p}_"))
        if written:
            S.total = fresh_int("total")
        for c in callee.ensures:
            self.pc.append(self.sev(c, env, pre, S))
        for c in callee.keeps:
            self.pc.append(self.sev(c, env, pre) == self.sev(c, env, S))
        if "audit" in uses(self.defs, callee):
            if callee.emits:                       # the callee's spec says exactly what it appends
                S.audit = S.audit + [("ev", ctor, [fresh_int("any") if x == WILD else self.sev(x, env, pre)
                                                   for x in args]) for ctor, args in callee.emits]
            else:                                  # it emits something its spec does not describe
                S.audit = S.audit + [("opaque", callee.name)]
        return None

    def refuses_beyond_spec(self, d):
        """Can `d` refuse other than through its spec needs? (a body need, or a callee that refuses)"""
        return any(s[0] == "need" for s in d.body) or \
            any(fails(self.defs, self.defs[c]) for c in callees(self.defs, d))

    def refusal_point(self, cond):
        """A place where the call can refuse: assume it passes (success path), and remember the
        state it was reached in, for the `ok otherwise` goal."""
        self.refusals.append((list(self.pc), cond))
        self.pc.append(cond)

    def ok_goal(self):
        """`ok otherwise`: no refusal point after the spec needs can fail. Returns (goal, reason)."""
        if self.opaque_refusers:
            who = ", ".join(dict.fromkeys(self.opaque_refusers))
            return None, f"relies on {who}, whose spec does not say when it refuses (no `ok otherwise`)"
        reach = [z3.And(*pc, z3.Not(c)) for pc, c in self.refusals]
        return z3.Not(z3.Or(*reach)) if reach else z3.BoolVal(True), None

    def emit_goal(self, d, env, pre, S):
        """The appended events must be exactly d.emits, in order. Returns (z3 goal, reason-if-unknowable)."""
        opaque = [e[1] for e in S.audit if e[0] == "opaque"]
        if opaque:
            return None, f"relies on {', '.join(dict.fromkeys(opaque))}, whose spec does not say what it emits"
        if len(S.audit) != len(d.emits):
            return z3.BoolVal(False), None
        conds = [z3.BoolVal(True)]
        for (ctor, args), got in zip(d.emits, S.audit):
            if got[1] != ctor or len(got[2]) != len(args):
                return z3.BoolVal(False), None
            conds += [_ev_eq(self.sev(x, env, pre), v) for x, v in zip(args, got[2]) if x != WILD]
        return z3.And(*conds), None

    def prove(self, d):
        self.relied_on = set()
        pre = Sym(z3.Array("bal", z3.IntSort(), z3.IntSort()), z3.Array("own", z3.IntSort(), z3.IntSort()),
                  z3.Int("total"))
        env = {p: z3.Int(p) for p, _ in d.params}
        params = dict(env)
        self.pc, S = [], pre.copy()
        self.refusals, self.opaque_refusers = [], []
        own_pc = []                                # the definition's own needs, without callee specs
        for i, s in enumerate(d.needs + d.body):
            if s[0] == "need":
                own_pc.append(self.sev(s[1], env, S))
                if i < len(d.needs):               # spec needs are the stated refusals
                    self.pc.append(own_pc[-1])
                else:                              # a body need may only fire if the spec says so
                    self.refusal_point(own_pc[-1])
            elif s[0] == "let":
                env[s[1]] = self.sev(s[2], env, S)
            else:
                self.sev(s[1], env, S)
        results = []
        goals = [("keep", c, code(c), self.sev(c, env, pre) == self.sev(c, env, S)) for c in d.keeps] + \
                [("ensure", c, code(c), self.sev(c, env, pre, S)) for c in d.ensures]
        premise = {}                               # goal kind -> what it may assume (default: success)
        unknowable = []
        if d.complete:
            g, why_unknown = self.ok_goal()
            if why_unknown:
                unknowable.append({"kind": "ok", "expr": None, "clause": "otherwise", "status": "unknown",
                                   "reason": why_unknown})
            else:
                goals.insert(0, ("ok", None, "otherwise", g))
                premise["ok"] = []                 # it is about the runs that refuse
        if d.emits:
            g, why_unknown = self.emit_goal(d, env, pre, S)
            text = ", then ".join(emit_code(ev) for ev in d.emits)
            if why_unknown:
                unknowable.append({"kind": "emit", "expr": d.emits, "clause": text, "status": "unknown",
                                   "reason": why_unknown})
            else:
                goals.append(("emit", d.emits, text, g))
        vacuous = self.vacuity(own_pc)
        if vacuous:
            out = [{"kind": kind, "expr": c, "clause": text, "status": "vacuous", "reason": vacuous}
                   for kind, c, text, _ in goals]
            out += [dict(u, status="vacuous", reason=vacuous) for u in unknowable]
            return out, sorted(self.relied_on)
        for kind, c, text, g in goals:
            assumed = premise.get(kind, self.pc)
            solver = z3.Solver()
            solver.set("timeout", 5000)
            solver.add(*assumed, z3.Not(g))
            r = solver.check()
            if r == z3.unsat:
                results.append({"kind": kind, "expr": c, "clause": text, "status": "proven"})
            elif r == z3.sat:
                nice = z3.Solver()
                nice.add(*assumed, z3.Not(g))
                nice.add(pre.now >= 0, pre.now <= 1000)
                for p, t in d.params:
                    v = params[p]
                    nice.add(v >= 0, v <= 1000) if t == "Int" else nice.add(v >= 0, v <= 3)
                    if t == "Account":
                        nice.add(z3.Select(pre.bal, v) >= 0, z3.Select(pre.bal, v) <= 1000,
                                 z3.Select(pre.own, v) >= 0, z3.Select(pre.own, v) <= 1)
                cx = self.find_cex(d, nice, solver, params, pre, kind)
                # a counterexample the interpreter does not reproduce is not evidence of a bug
                results.append({"kind": kind, "expr": c, "clause": text, "counterexample": cx,
                                "status": "refuted" if cx["confirmed_by_interpreter"] else "unconfirmed"})
            else:
                results.append({"kind": kind, "expr": c, "clause": text, "status": "unknown"})
        results += unknowable
        return results, sorted(self.relied_on)

    def vacuity(self, own_pc):
        """If no input can succeed, every guarantee holds trivially. Say so, and say whose fault."""
        s = z3.Solver()
        s.set("timeout", 5000)
        s.add(*self.pc)
        if s.check() != z3.unsat:
            return None
        s = z3.Solver()
        s.set("timeout", 5000)
        s.add(*own_pc)
        if s.check() == z3.unsat:
            return "no input passes its own needs, so the call never succeeds"
        who = ", ".join(sorted(self.relied_on))
        return f"no input succeeds given the specs of {who} (they contradict each other or its needs)"

    def find_cex(self, d, nice, solver, params, pre, kind=None):
        """A modular proof only knows a callee's spec, so the first model may pick inputs on which a
        callee's *body* refuses (e.g. an empty account). If replay does not confirm it, ask for a
        model with well-funded accounts and small amounts, which callee bodies rarely refuse.
        Whichever is used, the page reports whether the interpreter confirmed it."""
        if nice.check() != z3.sat:
            return self.render_cex(d, solver.model(), params, pre, kind)
        cx = self.render_cex(d, nice.model(), params, pre, kind)
        if cx["confirmed_by_interpreter"]:
            return cx
        opt = z3.Optimize()
        opt.add(*nice.assertions())
        for p, t in d.params:
            if t == "Account":
                opt.maximize(z3.Select(pre.bal, params[p]))
            else:
                opt.minimize(params[p])
        if opt.check() == z3.sat:
            cx2 = self.render_cex(d, opt.model(), params, pre, kind)
            if cx2["confirmed_by_interpreter"]:
                return cx2
        return cx

    def render_cex(self, d, m, params, pre, kind=None):
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
        now = m.eval(pre.now, model_completion=True).as_long()
        w = World.from_example(world, now)
        interp = Interp(self.defs)
        before = copy.deepcopy(w)
        outcome, after = interp.run(d.name, args, w)
        world_src = " ".join(f"{a}:{b}@{o}" for a, (b, o) in world.items()) + (f" now:{now}" if now else "")
        after_src = " ".join(f"{a}:{after.bal[a]}" for a in world)
        out_src = "ok" if outcome == ("ok",) else " ".join([outcome[0], *map(str, outcome[1])])
        if kind == "ok":                           # it must refuse although the spec needs hold
            violated = interp.unstated_refusal(d, args, before, outcome)
        else:
            violated = interp.holds(d, args, before, after) if outcome == ("ok",) else []
        emitted = ", ".join(" ".join(map(str, ev)) if isinstance(ev, tuple) else str(ev)
                            for ev in after.audit) if outcome == ("ok",) else ""
        return {"eg": f"eg {world_src} | {d.name} {' '.join(map(str, args))} -> {out_src} | {after_src}",
                "emitted": emitted or "nothing", "confirmed_by_interpreter": bool(violated)}

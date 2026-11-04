"""Symbolic execution to Z3. Calls to other definitions are verified *modularly*: the
prover uses only the callee's spec (needs, ensures, keeps), never its body — the same
"read the interface, not the code" rule the agent follows."""
from __future__ import annotations

import copy
import itertools

import z3

from .infer import ANY_ACCOUNT, callees, fails, ledger_writes, lifted, relevant_states, state_writes, uses, writes
from .run import assumed_op, channel_op, emit_channels, state_op
from .run import Interp, KernError, World
from .syntax import code, emit_code, flat

_fresh = itertools.count()


class Sym:
    """Symbolic state. `audit` holds only the events appended during the call being proved
    (the handler can only append): ("ev", Ctor, [z3 args]) or ("opaque", callee) when a callee
    emits events its spec does not describe."""

    def __init__(self, bal, own, total, audit=(), now=None, maps=None):
        self.bal, self.own, self.total, self.audit = bal, own, total, list(audit)
        self.now = now if now is not None else z3.Int("now")   # the call's time; never changes
        self.maps = dict(maps or {})                            # declared state -> z3 Array Int -> Int
        self.out = {}                                           # declared channel -> events, like audit

    def copy(self):
        c = Sym(self.bal, self.own, self.total, self.audit, self.now, self.maps)
        c.out = {ch: list(e) for ch, e in self.out.items()}
        return c

    def events(self, ch):
        return self.audit if ch == "audit" else self.out.get(ch, [])

    def append(self, ch, evs):
        if ch == "audit":
            self.audit = self.audit + evs
        else:
            self.out[ch] = self.out.get(ch, []) + evs


WILD = ("name", "_", False)


def _ev_eq(x, y):
    """Z3 condition for two symbolic values (ints or constructor values) being equal."""
    if isinstance(x, tuple) or isinstance(y, tuple):
        if not (isinstance(x, tuple) and isinstance(y, tuple)) or x[1] != y[1] or len(x[2]) != len(y[2]):
            return z3.BoolVal(False)
        return z3.And(*[z3.BoolVal(True)] + [_ev_eq(a, b) for a, b in zip(x[2], y[2])])
    return x == y


class Path:
    """One way through a body: path condition, state, bindings, refusal points, own conditions."""

    def __init__(self, pc, S, env, refusals, opaque, own, draws=None):
        self.pc, self.S, self.env, self.refusals, self.opaque, self.own = pc, S, env, refusals, opaque, own
        self.draws = list(draws or [])            # (handler, result term) in call order, for replay

    def fork(self, cond):
        return Path(self.pc + [cond], self.S.copy(), dict(self.env), list(self.refusals), list(self.opaque),
                    self.own + [cond], self.draws)


def _array_entries(m, v, limit=50):
    """(key, value) pairs a model's array value spells out explicitly: Store chains, and the entry
    table of an as-array function. The default (else) value is not a key."""
    out = []
    try:
        while z3.is_store(v) and len(out) < limit:
            out.append((v.arg(1).as_long(), v.arg(2).as_long()))
            v = v.arg(0)
        if z3.is_as_array(v):
            fi = m[z3.get_as_array_func(v)]
            for i in range(min(fi.num_entries(), limit)):
                e = fi.entry(i)
                out.append((e.arg_value(0).as_long(), e.value().as_long()))
    except Exception:
        return out
    return out


def _loop_offset(e, var):
    """c if e is the loop variable plus a constant (k, k + c, k - c), else None."""
    if e == ("name", var, False):
        return 0
    if e[0] == "bin" and e[1] in ("+", "-") and e[2] == ("name", var, False) and e[3][0] == "int":
        return e[3][1] if e[1] == "+" else -e[3][1]
    return None


REPLAY_BUDGET = 200_000     # loop iterations + quantifier steps one counterexample replay may take


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
        if k == "all":                           # E for every j in [A, B)
            j = z3.Int(f"{e[1]}_{next(_fresh)}")
            lo, hi = self.sev(e[2], env, cur, post), self.sev(e[3], env, cur, post)
            body = self.sev(e[4], dict(env, **{e[1]: j}), cur, post)
            return z3.ForAll([j], z3.Implies(z3.And(lo <= j, j < hi), body))
        if k == "bin":
            l, r = self.sev(e[2], env, cur, post), self.sev(e[3], env, cur, post)
            op = e[1]
            return {"+": lambda: l + r, "-": lambda: l - r, "*": lambda: l * r, "/": lambda: l / r, "=": lambda: l == r, "!=": lambda: l != r,
                    ">=": lambda: l >= r, "<=": lambda: l <= r, ">": lambda: l > r, "<": lambda: l < r,
                    "and": lambda: z3.And(l, r), "or": lambda: z3.Or(l, r),
                    "when": lambda: z3.Implies(r, l)}[op]()
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
        if channel_op(self.defs, name):
            S.append(channel_op(self.defs, name), [a[0] if isinstance(a[0], tuple) else ("opaque", name)])
            return None
        h = assumed_op(self.defs, name)
        if h:                                    # an observation: any value its assumed spec allows,
            env = {p: v for (p, _), v in zip(h.params, a)}       # a fresh one per call
            for n in h.needs:                     # the caller must meet it: a goal at this call site
                c = self.sev(n[1], env, S)
                self.loop_goals.append(("pre", (name, n[1]), f"{name}: need {code(n[1])} (at a call)", [(c, list(self.pc))]))
                self.pc.append(c)
            r = fresh_int(name.replace(".", "_"))
            self.draws.append((name, r))
            for c in h.ensures:
                self.pc.append(self.sev(c, dict(env, result=r), S, S))
            self.relied_on.add(f"assumed {name}")
            return r
        st = state_op(self.defs, name)
        if st:
            if st[1] == "get":
                return z3.Select(S.maps[st[0]], a[0])
            S.maps[st[0]] = z3.Store(S.maps[st[0]], a[0], a[1])
            return None
        if name in self.defs and self.defs[name].ret:
            return self.function_call(self.defs[name], a, S)
        if name in self.defs:
            return self.modular_call(self.defs[name], a, S)
        if name[:1].isupper():
            return ("ev", name, a)          # constructor value
        if name == "distinct":
            return z3.Distinct(*a)
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
        all_written = writes(self.defs, callee)   # inferred frame: everything else is unchanged
        written = ledger_writes(all_written)
        if ANY_ACCOUNT in written:
            S.bal = z3.Array(f"bal{next(_fresh)}", z3.IntSort(), z3.IntSort())
        for p in sorted(written - {ANY_ACCOUNT}):
            S.bal = z3.Store(S.bal, env[p], fresh_int(f"bal_{p}_"))
        if written:
            S.total = fresh_int("total")
        for x, ps in sorted(state_writes(all_written).items()):
            if ANY_ACCOUNT in ps:
                S.maps[x] = z3.Array(f"{x}{next(_fresh)}", z3.IntSort(), z3.IntSort())
                continue
            for p in sorted(ps):
                if p.startswith("["):              # the callee's loop wrote [lo, hi) only
                    lo_s, hi_s = p[1:-1].split(",")
                    lo, hi = (z3.IntVal(int(b)) if b.lstrip("-").isdigit() else env[b] for b in (lo_s, hi_s))
                    old, j = S.maps[x], z3.Int(f"j_{next(_fresh)}")
                    S.maps[x] = z3.Array(f"{x}_call{next(_fresh)}", z3.IntSort(), z3.IntSort())
                    self.pc.append(z3.ForAll([j], z3.Implies(z3.Or(j < lo, j >= hi),
                                                             z3.Select(S.maps[x], j) == z3.Select(old, j))))
                    for g_text, g in self._module_invs_over({x}, S):   # the callee keeps them (relied on)
                        self.pc.append(g)
                        self.relied_on.add(f"invariant on {x}")
                    continue
                S.maps[x] = z3.Store(S.maps[x], env[p], fresh_int(f"{x}_{p}_"))
        for c in callee.ensures:
            self.pc.append(self.sev(c, env, pre, S))
        for var, t, cond in self.invariants():    # the callee keeps the invariant at its keys
            for (p, pt) in callee.params:
                if pt == t:
                    self.pc.append(self.sev(cond, {var: env[p]}, S))
        for c in callee.keeps:
            self.pc.append(self.sev(c, env, pre) == self.sev(c, env, S))
        said = emit_channels(callee)
        for ch in ["audit"] + getattr(self.defs, "channels", []):
            if ch not in uses(self.defs, callee):
                continue
            if ch not in said:                     # it emits something its spec does not describe
                S.append(ch, [("opaque", callee.name)])
                continue
            for i, (ctor, args, *cond) in enumerate(callee.emits):   # its spec says exactly what
                if callee.emit_channel.get(i, "audit") != ch:
                    continue
                vals = [fresh_int("any") if x == WILD else self.sev(x, env, pre) for x in args]
                # a conditional event is appended only when its condition held before the call
                S.append(ch, [("ev", ctor, vals, self.sev(cond[0], env, pre)) if cond else ("ev", ctor, vals)])
        return None

    def function_call(self, f, args, S):
        """A pure function, modularly: the same arguments give the same result (an uninterpreted
        function), and the result satisfies the function's ensures. Its body is never read."""
        self.relied_on.add(f.name)
        fn = z3.Function(f"fn_{f.name}", *([z3.IntSort()] * (len(args) + 1)))
        r = fn(*args)
        env = dict({p: v for (p, _), v in zip(f.params, args)}, result=r)
        for c in f.ensures:
            self.pc.append(self.sev(c, env, S, S))
        return r

    def refuses_beyond_spec(self, d):
        """Can `d` refuse other than through its spec needs? (a body need, or a callee that refuses)"""
        return any(s[0] == "need" for s in flat(d.body)) or \
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

    def emit_goal(self, d, env, pre, S, ch="audit"):
        """The events appended on ch must be exactly d's emits for ch, in order.
        Returns (z3 goal, reason-if-unknowable)."""
        events = S.events(ch)
        emits = [ev for i, ev in enumerate(d.emits) if d.emit_channel.get(i, "audit") == ch]
        opaque = [e[1] for e in events if e[0] == "opaque"]
        if "a loop" in opaque:
            return None, "it emits events inside a loop, which a spec can't describe yet"
        if opaque:
            return None, f"relies on {', '.join(dict.fromkeys(opaque))}, whose spec does not say what it emits"
        if any(len(ev) > 2 for ev in emits) or any(len(a) > 3 for a in events):
            return self.conditional_emit_goal(emits, env, pre, events), None
        if len(events) != len(emits):
            return z3.BoolVal(False), None
        conds = [z3.BoolVal(True)]
        for (ctor, args), got in zip(emits, events):
            if got[1] != ctor or len(got[2]) != len(args):
                return z3.BoolVal(False), None
            conds += [_ev_eq(self.sev(x, env, pre), v) for x, v in zip(args, got[2]) if x != WILD]
        return z3.And(*conds), None

    def conditional_emit_goal(self, emits, env, pre, events):
        """Some events are conditional. The events whose conditions hold, in order, must be the same
        sequence on both sides: equal counts, and the k-th present event of each side match."""
        one = lambda c: z3.If(c, 1, 0)
        want = [(ctor, args, self.sev(c[0], env, pre) if c else z3.BoolVal(True)) for ctor, args, *c in emits]
        got = [(a[1], a[2], a[3] if len(a) > 3 else z3.BoolVal(True)) for a in events]
        def prefix(xs):                  # position of each event among the present ones, built incrementally
            out, acc = [], z3.IntVal(0)  # (rebuilding every prefix sum was O(n^2) terms; kern-90 profiled it)
            for *_, c in xs:
                out.append(acc)
                acc = acc + one(c)
            return out, acc
        (pos_w, n_w), (pos_g, n_g) = prefix(want), prefix(got)
        conds = [n_w == n_g]
        wanted = [[None if x == WILD else self.sev(x, env, pre) for x in args] for _, args, _ in want]
        for i, (ctor, args, cw) in enumerate(want):
            for j, (gctor, vals, cg) in enumerate(got):
                if gctor != ctor or len(vals) != len(args):
                    same = z3.BoolVal(False)
                else:
                    same = z3.And(*[z3.BoolVal(True)] + [_ev_eq(x, v) for x, v in zip(wanted[i], vals)
                                                         if x is not None])
                conds.append(z3.Implies(z3.And(cw, cg, pos_w[i] == pos_g[j]), same))
        return z3.And(*conds)

    def invariants(self):
        return getattr(self.defs, "invariants", [])

    def _stores_of(self, cond):
        from .infer import _names
        names = _names(cond)
        out = {n[:-4] for n in names if n.endswith(".get")}
        return out | ({"ledger"} if names & {"ledger.balance", "ledger.total"} else set())

    def invariant_touched(self, d, cond):
        """None if d writes nothing the invariant reads, "any" if it writes a computed key there,
        else True (its writes are all at parameters, where the invariant is then checked)."""
        from .infer import _names
        reads = _names(cond)
        ws = writes(self.defs, d)
        hit = False
        if reads & {"ledger.balance", "ledger.total"} and ledger_writes(ws):
            hit = True
            if ANY_ACCOUNT in ledger_writes(ws):
                return "any"
        for x, ps in state_writes(ws).items():
            if f"{x}.get" in reads:
                hit = True
                if ANY_ACCOUNT in ps:
                    return "any"
        return True if hit else None

    def bind(self, p):
        """Point the per-path bookkeeping (used by modular_call and refusal_point) at path p."""
        self.pc, self.refusals, self.opaque_refusers, self.draws = p.pc, p.refusals, p.opaque, p.draws

    def exec(self, stmts, path):
        """Symbolically run statements; an `if` forks the path. Returns the finished paths."""
        paths = [path]
        for s in stmts:
            nxt = []
            for p in paths:
                self.bind(p)
                if s[0] == "if":
                    c = self.sev(s[1], p.env, p.S)
                    nxt += self.exec(s[2], p.fork(c)) + self.exec(s[3], p.fork(z3.Not(c)))
                    continue
                if s[0] == "for":
                    self.exec_loop(s, p)
                    self.bind(p)
                    nxt.append(p)
                    continue
                if s[0] == "need":                 # a body need may only fire if the spec says so
                    v = self.sev(s[1], p.env, p.S)
                    p.own.append(v)
                    self.refusal_point(v)
                elif s[0] == "let":
                    p.env[s[1]] = self.sev(s[2], p.env, p.S)
                else:
                    self.sev(s[1], p.env, p.S)
                nxt.append(p)
            paths = nxt
        return paths

    # --- loops --------------------------------------------------------------------------------

    def _loop_effects(self, body, env, S):
        """What one run of a loop body can change: ledger accounts (z3 values), maps written only at
        the loop key (framed by range), maps written elsewhere (fully havocked), channels emitted on."""
        from .syntax import flat
        acc, ranged, full, chans = [], set(), set(), set()
        for s in flat(body):
            if s[0] != "do" or s[1][0] != "app":
                continue
            name, args = s[1][1][0], s[1][2]
            st = state_op(self.defs, name)
            if name in ("ledger.debit", "ledger.credit"):
                acc.append(args[0])
            elif st and st[1] == "set":
                off = _loop_offset(args[0], self._loop_var)
                if off is None:
                    full.add(st[0])
                else:                               # written at k + c: a shifted frame
                    ranged.add((st[0], off))
            elif name == "audit.emit":
                chans.add("audit")
            elif channel_op(self.defs, name):
                chans.add(channel_op(self.defs, name))
            elif name in self.defs:
                callee = self.defs[name]
                cenv = {p: a for (p, _), a in zip(callee.params, args)}
                for w in writes(self.defs, callee):
                    x, dot, q = w.rpartition(".")
                    if not dot:
                        acc.append(cenv[q] if q != ANY_ACCOUNT else None)
                    else:
                        full.add(x)
                chans |= {u for u in uses(self.defs, callee) if u == "audit" or u in getattr(self.defs, "channels", [])}
        shifts = {}
        for x, off in ranged:
            shifts.setdefault(x, set()).add(off)
        one = {x: next(iter(o)) for x, o in shifts.items() if len(o) == 1 and x not in full}
        return acc, one, full | {x for x in shifts if x not in one}, chans

    def _havoc(self, S, eff, env, lo, K, empty=None):
        """A state after some iterations: written stores fresh, maps written at the loop key equal
        to S outside [lo, K) (the inferred frame). With `empty` (no iteration ran), everything equals S.
        Returns (state, axioms)."""
        acc, ranged, full, _ = eff
        H, ax = S.copy(), []
        for a in acc:
            if a is None:
                H.bal = z3.Array(f"bal{next(_fresh)}", z3.IntSort(), z3.IntSort())
            else:
                H.bal = z3.Store(H.bal, self.sev(a, env, S), fresh_int("bal_loop_"))
        if acc:
            H.total = fresh_int("total")
        for x in set(ranged) | full:
            H.maps[x] = z3.Array(f"{x}_loop{next(_fresh)}", z3.IntSort(), z3.IntSort())
            if x in ranged:
                c, j = ranged[x], z3.Int(f"j_{next(_fresh)}")
                ax.append(z3.ForAll([j], z3.Implies(z3.Or(j < lo + c, j >= K + c),
                                                    z3.Select(H.maps[x], j) == z3.Select(S.maps[x], j))))
        if empty is not None:                      # a loop that never ran changed nothing
            same = [H.bal == S.bal, H.total == S.total] + [H.maps[x] == S.maps[x] for x in set(ranged) | full]
            ax.append(z3.Implies(empty, z3.And(*same)))
        return H, ax

    def _inv(self, invs, var, kval, env, S):
        return [self.sev(c, dict(env, **{var: kval}), self.call_pre, S) for c in invs]

    def _module_invs_over(self, stores, S):
        """Module invariants that read a store the loop writes, for every key: kept automatically."""
        from .infer import _names
        out = []
        for var, t, cond in self.invariants():
            reads = _names(cond)
            if any(f"{x}.get" in reads for x in stores) or ("ledger" in stores and reads & {"ledger.balance", "ledger.total"}):
                j = z3.Int(f"m_{next(_fresh)}")
                out.append((f"module invariant {var}:{t} {code(cond)}", z3.ForAll([j], self.sev(cond, {var: j}, S))))
        return out

    def exec_loop(self, s, p):
        """for k from LO to HI with invariants: on entry (k = LO), kept by an arbitrary iteration
        (LO <= k < HI, invariants assumed, written stores havocked), and after (k = max(LO, HI))."""
        var, lo_e, hi_e, invs, body = s[1], s[2], s[3], s[4], s[5]
        lo, hi = self.sev(lo_e, p.env, p.S), self.sev(hi_e, p.env, p.S)
        self._loop_var = var
        eff = self._loop_effects(body, p.env, p.S)
        texts = [code(c) for c in invs]
        stores = set(eff[1]) | eff[2] | ({"ledger"} if eff[0] else set())
        # entry
        entry = self._inv(invs, var, lo, p.env, p.S) + [g for _, g in self._module_invs_over(stores, p.S)]
        names = list(zip(texts, invs)) + [(t, ("module", t)) for t, _ in self._module_invs_over(stores, p.S)]
        for (text, ast), g in zip(names, entry):
            self.loop_goals.append(("loop", ("entry", ast), f"{text}  (holds when the loop starts)", [(g, list(p.pc))]))
        # an arbitrary iteration
        k = z3.Int(f"{var}_{next(_fresh)}")
        H, ax = self._havoc(p.S, eff, p.env, lo, k)
        head = self._inv(invs, var, k, p.env, H) + [g for _, g in self._module_invs_over(stores, H)]
        it = Path(p.pc + [lo <= k, k < hi] + ax + head, H, dict(p.env, **{var: k}), list(p.refusals), list(p.opaque),
                  p.own + [lo <= k, k < hi])
        n_ref, n_opq = len(p.refusals), len(p.opaque)
        for q in self.exec(body, it):
            kept = self._inv(invs, var, k + 1, q.env, q.S) + [g for _, g in self._module_invs_over(stores, q.S)]
            for (text, ast), g in zip(names, kept):
                self.loop_goals.append(("loop", ("kept", ast), f"{text}  (kept by every iteration)", [(g, q.pc)]))
            p.refusals += q.refusals[n_ref:]       # a refusal inside the loop is a refusal of the call
            p.opaque += q.opaque[n_opq:]
        # after the loop
        K = z3.If(hi >= lo, hi, lo)
        A, ax = self._havoc(p.S, eff, p.env, lo, K, empty=hi <= lo)
        p.pc += ax + self._inv(invs, var, K, p.env, A) + [g for _, g in self._module_invs_over(stores, A)]
        for ch in eff[3]:                          # events inside a loop are not described yet
            A.append(ch, [("opaque", "a loop")])
        p.S = A

    def prove(self, d, first_kill=False):
        """Prove every guarantee of d. With first_kill (mutation testing), stop at the first
        confirmed refutation: the caller only needs to know whether the mutant was caught."""
        self.relied_on = set()
        pre = Sym(z3.Array("bal", z3.IntSort(), z3.IntSort()), z3.Array("own", z3.IntSort(), z3.IntSort()),
                  z3.Int("total"), maps={x: z3.Array(f"map_{x}", z3.IntSort(), z3.IntSort())
                                         for x in getattr(self.defs, "states", {})})
        env = {p: z3.Int(p) for p, _ in d.params}
        params = dict(env)
        start = Path([], pre.copy(), env, [], [], [])
        self.bind(start)
        self.call_pre, self.loop_goals = pre, []
        from .syntax import flat as _flat
        if any(s[0] == "for" for s in _flat(d.body)) or any(
                "[" in w for c in callees(self.defs, d) for w in writes(self.defs, self.defs[c])):
            for var, t, cond in self.invariants():   # held before the call at every key, provided every
                j = z3.Int(f"pre_{next(_fresh)}")      # writer of the store keeps it: relied on, like a callee
                self.pc.append(z3.ForAll([j], self.sev(cond, {var: j}, pre)))
                self.relied_on |= {f"invariant on {x}" for x in self._stores_of(cond)}
        for var, t, cond in self.invariants():    # it held before the call, for every key; we use
            for p, pt in d.params:                # it for the keys the call can name
                if pt == t:
                    self.pc.append(self.sev(cond, {var: env[p]}, pre))
        for n in d.needs:                          # spec needs are the stated refusals
            v = self.sev(n[1], env, start.S)
            start.own.append(v)
            self.pc.append(v)
        paths = self.exec(d.body, start)
        results = []
        # per goal: one (goal, premise) per path; it holds iff it holds on every path
        def per_path(fn):
            out = []
            for p in paths:
                self.bind(p)                       # a function call in a clause adds to p's premise
                out.append((fn(p), p.pc, p.draws))
            return out
        goals = [("keep", c, code(c), per_path(lambda p: self.sev(c, p.env, pre) == self.sev(c, p.env, p.S)))
                 for c in d.keeps] + \
                [("ensure", c, code(c), per_path(lambda p: self.sev(c, p.env, pre, p.S))) for c in d.ensures]
        unknowable = []
        goals += [(kind, c, text, cases) for kind, c, text, cases in self.loop_goals]
        for var, t, cond in self.invariants():
            touched = self.invariant_touched(d, cond)
            if touched is None:
                continue
            text = f"{var}:{t} {code(cond)}"
            if touched == "any":
                unknowable.append({"kind": "invariant", "expr": (var, t, cond), "clause": text, "status": "unknown",
                                   "reason": "it writes a computed key, so the invariant can't be checked key by key"})
                continue
            goals.append(("invariant", (var, t, cond), text,
                          [(z3.And(*[z3.BoolVal(True)] + [self.sev(cond, {var: p.env[q]}, p.S)
                                                           for q, qt in d.params if qt == t]), p.pc) for p in paths]))
        if d.complete:
            self.refusals = [r for p in paths for r in p.refusals]
            self.opaque_refusers = [o for p in paths for o in p.opaque]
            g, why_unknown = self.ok_goal()
            if why_unknown:
                unknowable.append({"kind": "ok", "expr": None, "clause": "otherwise", "status": "unknown",
                                   "reason": why_unknown})
            else:                                  # it is about the runs that refuse: no success premise
                goals.insert(0, ("ok", None, "otherwise", [(g, [])]))
        for ch in emit_channels(d):                # one exact sequence per channel; order across channels
            emits = [ev for i, ev in enumerate(d.emits) if d.emit_channel.get(i, "audit") == ch]   # is not said
            emit_cases = per_path(lambda p: self.emit_goal(d, p.env, pre, p.S, ch))
            text = ("" if ch == "audit" else f"{ch} ") + ", then ".join(emit_code(ev) for ev in emits)
            why_unknown = next((w for (g, w), _, _ in emit_cases if w), None)
            if why_unknown:
                unknowable.append({"kind": "emit", "expr": emits, "clause": text, "status": "unknown",
                                   "reason": why_unknown, "channel": ch})
            else:
                goals.append(("emit", emits if ch == "audit" else ("channel", ch, emits), text,
                              [(g, pc, dr) for (g, _), pc, dr in emit_cases]))
        vacuous = self.vacuity(paths)
        if vacuous:                                # loop goals have premises of their own: still solved
            if any(g[0] == "loop" for g in self.loop_goals):
                vacuous = ("no input gets past a loop: its invariants can't all hold when it ends "
                           "(see the loop's own goals)")
            out = [self.solve(d, kind, c, text, cases, params, pre) if kind in ("loop", "pre") else
                   {"kind": kind, "expr": c, "clause": text, "status": "vacuous", "reason": vacuous}
                   for kind, c, text, cases in goals]
            out += [dict(u, status="vacuous", reason=vacuous) for u in unknowable]
            return out, sorted(self.relied_on)
        for kind, c, text, cases in goals:
            results.append(self.solve(d, kind, c, text, cases, params, pre))
            if first_kill and results[-1]["status"] == "refuted":
                return results, sorted(self.relied_on)
        results += unknowable
        return results, sorted(self.relied_on)

    def solve(self, d, kind, c, text, cases, params, pre):
        unknown = False
        for g, assumed, *rest in cases:
            self._replay_draws = rest[0] if rest else []
            solver = z3.Solver()
            solver.set("timeout", 5000)
            solver.add(*assumed, z3.Not(g))
            r = solver.check()
            if r == z3.unsat:
                continue
            if r != z3.sat:
                unknown = True
                continue
            nice = z3.Solver()
            nice.set("timeout", 5000)              # every search has a time limit (FZ finding 1)
            nice.add(*assumed, z3.Not(g))
            nice.add(pre.now >= 0, pre.now <= 1000)
            for p, t in d.params:
                v = params[p]
                nice.add(v >= 0, v <= 1000) if t == "Int" else nice.add(v >= 0, v <= 3)
                if t == "Account":
                    nice.add(z3.Select(pre.bal, v) >= 0, z3.Select(pre.bal, v) <= 1000,
                             z3.Select(pre.own, v) >= 0, z3.Select(pre.own, v) <= 1)
                for x, arr in pre.maps.items():
                    if self.defs.states[x] == t:
                        nice.add(z3.Select(arr, v) >= 0, z3.Select(arr, v) <= 1000)
            cx = self.find_cex(d, nice, solver, params, pre, kind)
            # a counterexample the interpreter does not reproduce is not evidence of a bug
            return {"kind": kind, "expr": c, "clause": text, "counterexample": cx,
                    "status": "refuted" if cx["confirmed_by_interpreter"] else "unconfirmed"}
        return {"kind": kind, "expr": c, "clause": text, "status": "unknown" if unknown else "proven"}

    def vacuity(self, paths):
        """If no input can succeed, every guarantee holds trivially. Say so, and say whose fault."""
        def unsat(cs):
            s = z3.Solver()
            s.set("timeout", 5000)
            s.add(*cs)
            return s.check() == z3.unsat
        if not all(unsat(p.pc) for p in paths):
            return None
        if all(unsat(p.own) for p in paths):
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
        if any("forall" in a.sexpr() for a in nice.assertions()):
            # Z3 can't optimise over quantified constraints: try a few other models instead, each
            # differing from the last in its parameters, until one replays
            small = z3.Solver()                    # real bugs nearly always show in a small world
            small.set("timeout", 5000)
            small.add(*nice.assertions())
            for p, t in d.params:
                if t == "Int":
                    small.add(params[p] >= 0, params[p] <= 5)
            if small.check() == z3.sat:
                cx2 = self.render_cex(d, small.model(), params, pre, kind)
                if cx2["confirmed_by_interpreter"]:
                    return cx2
            m = nice.model()
            for _ in range(8):
                nice.add(z3.Or(*[params[p] != m.eval(params[p], model_completion=True) for p, _ in d.params]))
                if nice.check() != z3.sat:
                    break
                m = nice.model()
                cx2 = self.render_cex(d, m, params, pre, kind)
                if cx2["confirmed_by_interpreter"]:
                    return cx2
            return cx
        opt = z3.Optimize()
        opt.set("timeout", 5000)
        opt.add(*nice.assertions())
        for p, t in d.params:                      # a refusal is likeliest with little money; a broken
            if t == "Account":                     # guarantee with plenty (callee bodies rarely refuse)
                (opt.minimize if kind == "ok" else opt.maximize)(z3.Select(pre.bal, params[p]))
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
        maps, keys = {}, []                        # declared state at the keys the call can name,
        shown = relevant_states(self.defs, d)      # only the state the call or its spec can see
        for x, arr in pre.maps.items():
            if x not in shown:
                continue
            for p, t in d.params:
                if self.defs.states[x] != t:
                    continue
                zk = m.eval(params[p], model_completion=True).as_long()
                key = letters[zk] if t == "Account" else zk
                if (x, key) in keys:
                    continue
                keys.append((x, key))
                val = m.eval(z3.Select(arr, params[p]), model_completion=True).as_long()
                if val:
                    maps.setdefault(x, {})[key] = val
            if self.defs.states[x] == "Int":       # a quantified goal's witness key is no parameter:
                for zk, val in _array_entries(m, m.eval(arr, model_completion=True)):   # take every key the
                    if (x, zk) not in keys and abs(zk) <= 10**6:                  # model assigns
                        keys.append((x, zk))
                        if val:
                            maps.setdefault(x, {})[zk] = val
        w = World.from_example(world, now, maps)
        draws = [m.eval(t, model_completion=True).as_long() for _, t in getattr(self, "_replay_draws", [])]
        queue = list(draws)                        # the model's own draws, fed back in call order

        def provider(name, args):
            if queue:
                return queue.pop(0)
            from .run import PROVIDERS
            import random as _r
            return PROVIDERS[name](_r.Random(0), *args)
        interp = Interp(self.defs, provider=provider)
        interp.budget = REPLAY_BUDGET              # a model may pick a huge loop range: then unconfirmed
        before = w.copy()
        try:
            outcome, after = interp.run(d.name, args, w)
        except KernError as e:                     # e.g. a handler's need not met: an error, not a refusal
            world_src = " ".join(f"{a}:{b}@{o}" for a, (b, o) in world.items()) + (f" now:{now}" if now else "")
            world_src += "".join(f" {x}.{k}:{v}" for x, kv in maps.items() for k, v in kv.items())
            return {"eg": f"eg {world_src} | {d.name} {' '.join(map(str, args))} -> error: {e}",
                    "emitted": "nothing", "confirmed_by_interpreter": kind == "pre" and "need of" in str(e)}
        world_src = " ".join(f"{a}:{b}@{o}" for a, (b, o) in world.items()) + (f" now:{now}" if now else "")
        world_src += "".join(f" {x}.{k}:{v}" for x, kv in maps.items() for k, v in kv.items())
        after_src = " ".join(f"{a}:{after.bal[a]}" for a in world)
        after_src += "".join(f" {x}.{k}:{after.maps.get(x, {}).get(k, 0)}" for x, k in keys
                             if after.maps.get(x, {}).get(k, 0) != maps.get(x, {}).get(k, 0))
        out_src = "ok" if outcome == ("ok",) else " ".join([outcome[0], *map(str, outcome[1])])
        if kind == "loop":                         # replay with the loop invariants checked at each head
            interp.check_loops, interp.budget = True, REPLAY_BUDGET
            try:
                outcome, after = interp.run(d.name, args, before.copy())
                violated = bool(interp.loop_violations) or (outcome == ("ok",) and interp.broken_invariants(after)
                                                            and not interp.broken_invariants(before))
            except KernError:                      # too long to replay: not confirmed (FZ finding 5)
                violated = False
        elif kind == "ok":                         # it must refuse although the spec needs hold
            violated = interp.unstated_refusal(d, args, before, outcome)
        elif kind == "invariant":                  # it held before (at these keys) and not after
            violated = outcome == ("ok",) and interp.broken_invariants(after) and not interp.broken_invariants(before)
        else:
            violated = interp.holds(d, args, before, after) if outcome == ("ok",) else []
        emitted = ", ".join(" ".join(map(str, ev)) if isinstance(ev, tuple) else str(ev)
                            for ev in after.audit) if outcome == ("ok",) else ""
        return {"eg": f"eg {world_src} | {d.name} {' '.join(map(str, args))} -> {out_src} | {after_src}",
                "emitted": emitted or "nothing", "confirmed_by_interpreter": bool(violated)}

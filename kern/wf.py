"""Well-formedness and type checking, run before anything is proven or executed.

The prover and the interpreter only agree on well-formed, well-typed definitions, so
nothing reaches them without passing this check. It rejects:
  * unknown names, wrong arity, a variable applied to arguments
  * a `let` that shadows a parameter, an earlier `let`, a handler or a definition
  * specs that read body `let`s, call write/audit handlers or definitions, or
    use primes outside `ensure`; `_` outside the top level of an `emit` argument
  * recursion (no termination measures yet)
  * type errors: Int, Account, Owner, Bool, Event, Unit (e.g. `from + to`)
"""
from __future__ import annotations

from .infer import called_names, callees
from .run import HANDLERS, channel_op, is_handler, state_op
from .syntax import ParseError, code

TYPES = True                                  # step 3 of docs/FIXES.md ran with this off

PARAM_TYPES = {"Int", "Account"}
SIG = {                                       # handler -> (argument types, result type)
    "ledger.balance": (["Account"], "Int"),
    "ledger.owner": (["Account"], "Owner"),
    "ledger.total": ([], "Int"),
    "ledger.debit": (["Account", "Int"], "Unit"),
    "ledger.credit": (["Account", "Int"], "Unit"),
    "clock.now": ([], "Int"),
    "audit.emit": (["Event"], "Unit"),
}
SPEC_READS = {"ledger.balance", "ledger.owner", "ledger.total", "clock.now"}   # time is fixed per call
NO_AFTER_STATE = {"clock.now"}                                                 # ... so it has no `'`
ANY = "Any"
WILD = ("name", "_", False)


class WellFormedError(ParseError):
    def __init__(self, problems):
        self.problems = problems
        Exception.__init__(self, "not well-formed:\n  " + "\n  ".join(problems))


class _Checker:
    def __init__(self, defs, d):
        self.defs, self.d, self.out, self.used = defs, d, [], set()

    def err(self, where, msg):
        self.out.append(f"{self.d.name}: {where}: {msg}")

    # ---- expressions: return the type (or ANY after an error, to avoid cascades)
    def ty(self, e, scope, where, spec, primed_ok=False, wild_ok=False):
        k = e[0]
        if k == "int":
            return "Int"
        if k == "bin":
            op = e[1]
            l = self.ty(e[2], scope, where, spec, primed_ok)
            r = self.ty(e[3], scope, where, spec, primed_ok)
            if op in ("+", "-"):
                return self.need_types(where, op, (l, r), ("Int", "Int"), "Int")
            if op in ("*", "/"):             # linear arithmetic only, so the prover stays decidable
                if op == "/" and not (e[3][0] == "int" and e[3][1] > 0):
                    self.err(where, f"`{code(e)}`: `/` needs a positive number on the right")
                if op == "*" and e[2][0] != "int" and e[3][0] != "int":
                    self.err(where, f"`{code(e)}`: `*` needs a number on one side")
                return self.need_types(where, op, (l, r), ("Int", "Int"), "Int")
            if op in (">=", "<=", ">", "<"):
                return self.need_types(where, op, (l, r), ("Int", "Int"), "Bool")
            if op in ("and", "or"):
                return self.need_types(where, op, (l, r), ("Bool", "Bool"), "Bool")
            if op == "when":
                self.err(where, "`when` is only allowed at the end of an `ensure` or `emit`")
                return "Bool"
            if TYPES and ANY not in (l, r) and (l != r or l == "Unit"):
                self.err(where, f"`{code(e)}` compares {l} with {r}")
            return "Bool"
        if k == "name":
            name, primed = e[1], e[2]
            if e == WILD:
                if not wild_ok:
                    self.err(where, "`_` is only allowed as a top-level `emit` argument")
                return ANY
            return self.apply(name, primed, [], scope, where, spec, primed_ok, e)
        if k == "app":
            (name, primed), args = e[1], e[2]
            return self.apply(name, primed, args, scope, where, spec, primed_ok, e)
        if k == "all":
            self.err(where, "`for j from A to B` is only allowed at the end of an `ensure` or a loop `invariant`")
            return "Bool"
        self.err(where, f"cannot check {e}")
        return ANY

    def need_types(self, where, op, got, want, result):
        if TYPES and ANY not in got and tuple(got) != want:
            self.err(where, f"`{op}` needs {' and '.join(want)}, got {' and '.join(got)}")
        return result

    def apply(self, name, primed, args, scope, where, spec, primed_ok, e):
        if primed and not primed_ok:
            self.err(where, f"`{name}'` (after-state) is only allowed in `ensure`")
        arg_types = [self.ty(a, scope, where, spec, primed_ok) for a in args]
        if name in scope:
            if args:
                self.err(where, f"`{name}` is a value, not a function")
            if primed:
                self.err(where, f"`{name}'`: only handler reads have an after-state")
            return scope[name]
        if is_handler(self.defs, name):
            if primed and name in NO_AFTER_STATE:
                self.err(where, f"`{name}'`: the time is the same before and after a call")
            st = state_op(self.defs, name)
            reads = name in SPEC_READS or (st and st[1] == "get")
            if spec and not reads:
                self.err(where, f"specs may only read state; `{name}` has an effect")
            from .run import assumed_op
            h = assumed_op(self.defs, name)
            if h:                                         # an observation: not a pure read
                if spec:
                    self.err(where, f"specs cannot observe; `{name}` is an assumed handler")
                want, result = [t for _, t in h.params], "Int"
            elif channel_op(self.defs, name):
                want, result = ["Event"], "Unit"
            elif st:
                want, result = ([st[2]], "Int") if st[1] == "get" else ([st[2], "Int"], "Unit")
            else:
                want, result = SIG[name]
            return self.check_args(where, name, want, arg_types, result)
        if name in self.defs:
            fn = self.defs[name].ret                      # `-> Int`: a pure, total function
            if spec and not fn:
                self.err(where, f"specs cannot call definitions (`{name}`); only functions (`-> Int`)")
            if primed:
                self.err(where, f"`{name}'`: only handler reads have an after-state")
            want = [t for _, t in self.defs[name].params]
            return self.check_args(where, name, want, arg_types, "Int" if fn else "Unit")
        if name == "distinct":                        # `distinct a b c`: pairwise different
            if len(args) < 2 or len(set(t for t in arg_types if t != ANY)) > 1:
                self.err(where, "`distinct` takes two or more values of one type")
            return "Bool"
        if name[:1].isupper():
            for a, t in zip(args, arg_types):
                if TYPES and t == "Unit":
                    self.err(where, f"`{code(a)}` has no value (Unit) and cannot be a constructor argument")
            return "Event"
        self.err(where, f"unknown name `{name}`")
        return ANY

    def check_args(self, where, name, want, got, result):
        if len(want) != len(got):
            self.err(where, f"`{name}` takes {len(want)} argument(s), got {len(got)}")
            return result
        for i, (w, g) in enumerate(zip(want, got)):
            if TYPES and g != ANY and w != g:
                self.err(where, f"argument {i + 1} of `{name}` must be {w}, got {g}")
        return result

    def expect(self, where, t, want):
        if TYPES and t != ANY and t != want:
            self.err(where, f"must be {want}, got {t}")

    # ---- a whole definition
    def run(self):
        d = self.d
        params = {}
        for p, t in d.params:
            if t not in PARAM_TYPES:
                self.err("header", f"parameter type `{t}` is not supported (Int, Account)")
            if p in params:
                self.err("header", f"parameter `{p}` appears twice")
            params[p] = t if t in PARAM_TYPES else ANY
        if d.ret and d.ret != "Int":
            self.err("header", "a function returns Int")
        if d.ret:
            from .infer import uses
            if any(s[0] == "need" for s in d.needs + list(_flat(d.body))):
                self.err("header", "a function (-> Int) is total: no `need`")
            if uses(self.defs, d):
                self.err("header", "a function (-> Int) is pure: it may not read or change state")
            if not _assigns(d.body):
                self.err("body", "every path must set `result`")
            params["result"] = "Int"                      # specs (ensure) may name the result
        for n in d.needs:
            where = f"need {code(n[1])}"
            self.expect(where, self.ty(n[1], params, where, spec=True), "Bool")
            for a in n[3]:
                self.ty(a, params, where, spec=True)
        for c in d.keeps:
            t = self.ty(c, params, f"keep {code(c)}", spec=True)
            if TYPES and t in ("Unit", "Bool"):
                self.err(f"keep {code(c)}", f"must be a value, got {t}")
        for c in d.ensures:
            self.clause(c, params, f"ensure {code(c)}")
        for i, ch in d.emit_channel.items():
            if ch not in getattr(self.defs, "channels", []):
                self.err(f"emit {ch} {d.emits[i][0]}", f"no channel named `{ch}` (declare `channel {ch}`)")
        for ev in d.emits:
            where = f"emit {ev[0]}"
            for a in ev[1]:
                self.ty(a, params, where, spec=True, wild_ok=True)
            if len(ev) > 2:
                self.expect(where, self.ty(ev[2], params, where, spec=True), "Bool")
        self.block(d.body, dict(params))
        return self.out

    def clause(self, c, scope, where):
        """`E [when C] [for j from A to B]`: E may use after-state reads, C is about the before-state."""
        if c[0] == "all":
            for b in (c[2], c[3]):
                self.expect(where, self.ty(b, scope, where, spec=True), "Int")
            if c[1] in scope or c[1] in self.defs:
                self.err(where, f"`{c[1]}` shadows an existing name")
            scope = dict(scope, **{c[1]: "Int"})
            c = c[4]
        if c[0] == "bin" and c[1] == "when":
            self.expect(where, self.ty(c[2], scope, where, spec=True, primed_ok=True), "Bool")
            self.expect(where, self.ty(c[3], scope, where, spec=True), "Bool")
        else:
            self.expect(where, self.ty(c, scope, where, spec=True, primed_ok=True), "Bool")

    def block(self, stmts, scope, in_loop=False):
        for s in stmts:
            if s[0] == "inv":
                self.err(f"invariant {code(s[1])}", "an `invariant` line belongs at the start of a `for` loop")
            elif s[0] == "for":
                where = f"for {s[1]} from {code(s[2])} to {code(s[3])}"
                if in_loop:
                    self.err(where, "nested loops are not supported yet")
                for b in (s[2], s[3]):
                    self.expect(where, self.ty(b, scope, where, spec=False), "Int")
                if s[1] in scope or s[1] in self.used or is_handler(self.defs, s[1]) or s[1] in self.defs:
                    self.err(where, f"`{s[1]}` shadows an existing name")
                self.used.add(s[1])
                inner = dict(scope, **{s[1]: "Int"})
                for inv in s[4]:
                    self.clause(inv, inner, f"invariant {code(inv)}")
                self.block(s[5], inner, in_loop=True)
            elif s[0] == "if":
                where = f"if {code(s[1])}"
                self.expect(where, self.ty(s[1], scope, where, spec=False), "Bool")
                self.block(s[2], dict(scope), in_loop)    # a branch's names stay in the branch,
                self.block(s[3], dict(scope), in_loop)    # but are never reused (self.used)
            elif s[0] == "let":
                where = f"{s[1]} = {code(s[2])}"
                t = self.ty(s[2], scope, where, spec=False)
                if s[1] == "result" and self.d.ret:       # each path sets it once (checked by _assigns)
                    scope.pop("result", None)
                if s[1] in scope or s[1] in self.used or is_handler(self.defs, s[1]) or s[1] in self.defs:
                    self.err(where, f"`{s[1]}` shadows an existing name")
                if TYPES and t == "Unit":
                    self.err(where, "has no value (Unit)")
                scope[s[1]] = t
                if s[1] != "result":
                    self.used.add(s[1])
            elif s[0] == "need":
                where = f"need {code(s[1])}"
                self.expect(where, self.ty(s[1], scope, where, spec=False), "Bool")
                for a in s[3]:
                    t = self.ty(a, scope, where, spec=False)
                    if TYPES and t == "Unit":
                        self.err(where, f"failure payload `{code(a)}` has no value (Unit)")
            else:
                where = code(s[1])
                t = self.ty(s[1], scope, where, spec=False)
                if TYPES and t not in ("Unit", ANY):
                    self.err(where, f"value of type {t} is computed and discarded")


def _flat(stmts):
    from .syntax import flat
    return flat(stmts)


def _assigns(stmts):
    """Every path through stmts sets `result`."""
    for s in stmts:
        if s[0] == "let" and s[1] == "result":
            return True
        if s[0] == "if" and _assigns(s[2]) and _assigns(s[3]):
            return True
    return False


def problems(defs, d):
    """Problems with one definition (used for mutants as well as source)."""
    return _Checker(defs, d).run()


def cycles(defs):
    out, state = [], {}

    def visit(n, path):
        state[n] = "open"
        own = [n] if n in called_names(defs[n]) else []     # callees() leaves out self-calls
        for c in own + callees(defs, defs[n]):
            if state.get(c) == "open":
                loop = path[path.index(c):] + [c]
                out.append(f"{loop[0]}: recursive ({' -> '.join(loop)}); no termination measure yet")
            elif c not in state:
                visit(c, path + [c])
        state[n] = "done"

    for n in defs:
        if n not in state:
            visit(n, [n])
    return out


def invariant_problems(defs):
    from .syntax import Def
    out = []
    for var, t, cond in getattr(defs, "invariants", []):
        c = _Checker(defs, Def(f"invariant {var}", [], None))
        c.expect(code(cond), c.ty(cond, {var: t}, code(cond), spec=True), "Bool")
        out += c.out
        # Checked key by key (prover: at parameter keys; host: at written keys), which is only
        # sound if the invariant reads state at its own bound key and nowhere else.
        for e in _walk_expr(cond):
            name, args = (e[1], []) if e[0] == "name" else (e[1][0], e[2]) if e[0] == "app" else (None, None)
            if name is None or name == var or not is_handler(defs, name):
                continue
            if args != [("name", var, False)]:
                out.append(f"invariant {var}: {code(cond)}: `{code(e)}` must read state at `{var}` only")
    return out


def _walk_expr(e):
    yield e
    if e[0] == "bin":
        yield from _walk_expr(e[2])
        yield from _walk_expr(e[3])
    elif e[0] == "app":
        for a in e[2]:
            yield from _walk_expr(a)


def handler_problems(defs):
    """An assumed spec may read only its arguments and `result`: anything else (ledger or state reads)
    would let an assumption carry an unproven claim about owned state (kern-90)."""
    out = []
    for name, h in getattr(defs, "handlers", {}).items():
        if h.ret != "Int":
            out.append(f"handler {name}: must return Int (`-> Int`)")
        if h.keeps or h.emits or h.examples or h.complete:
            out.append(f"handler {name}: only `need` and `ensure` lines (an assumed spec)")
        allowed = {p for p, _ in h.params} | {"result"}
        for e in [n[1] for n in h.needs] + h.ensures:
            for x in _walk_expr(e):
                n = x[1] if x[0] == "name" else x[1][0] if x[0] == "app" else None
                if n and n not in allowed and not n[:1].isupper() and n != "distinct":
                    out.append(f"handler {name}: `{code(x)}`: an assumed spec may read only its arguments and `result`")
        c = _Checker(defs, h)
        for p, t in h.params:
            if t not in PARAM_TYPES:
                out.append(f"handler {name}: parameter type `{t}` is not supported")
        scope = dict(h.params, result="Int")
        for n in h.needs:
            c.expect(code(n[1]), c.ty(n[1], scope, code(n[1]), spec=True), "Bool")
        for e in h.ensures:
            c.clause(e, scope, code(e))
        out += c.out
    return out


def check_module(defs):
    found = [p for d in defs.values() for p in problems(defs, d)] + cycles(defs) + invariant_problems(defs) + \
        handler_problems(defs)
    if found:
        raise WellFormedError(found)
    return defs

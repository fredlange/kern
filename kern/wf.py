"""Well-formedness and type checking, run before anything is proven or executed.

The prover and the interpreter only agree on well-formed, well-typed definitions, so
nothing reaches them without passing this check. It rejects:
  * unknown names, wrong arity, a variable applied to arguments
  * a `let` that shadows a parameter, an earlier `let`, a handler or a definition
  * specs that read body `let`s, call write/clock/audit handlers or definitions, or
    use primes outside `ensure`; `_` outside the top level of an `emit` argument
  * recursion (no termination measures yet)
  * type errors: Int, Account, Owner, Bool, Event, Unit (e.g. `from + to`)
"""
from __future__ import annotations

from .infer import called_names, callees
from .run import HANDLERS
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
SPEC_READS = {"ledger.balance", "ledger.owner", "ledger.total"}
ANY = "Any"
WILD = ("name", "_", False)


class WellFormedError(ParseError):
    def __init__(self, problems):
        self.problems = problems
        Exception.__init__(self, "not well-formed:\n  " + "\n  ".join(problems))


class _Checker:
    def __init__(self, defs, d):
        self.defs, self.d, self.out = defs, d, []

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
            if op in (">=", "<=", ">", "<"):
                return self.need_types(where, op, (l, r), ("Int", "Int"), "Bool")
            if op in ("and", "or"):
                return self.need_types(where, op, (l, r), ("Bool", "Bool"), "Bool")
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
        if name in HANDLERS:
            if spec and name not in SPEC_READS:
                self.err(where, f"specs may only read state; `{name}` has an effect")
            want, result = SIG[name]
            return self.check_args(where, name, want, arg_types, result)
        if name in self.defs:
            if spec:
                self.err(where, f"specs cannot call definitions (`{name}`)")
            if primed:
                self.err(where, f"`{name}'`: only handler reads have an after-state")
            want = [t for _, t in self.defs[name].params]
            return self.check_args(where, name, want, arg_types, "Unit")
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
        if d.ret:
            self.err("header", "return values are not supported yet")
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
            where = f"ensure {code(c)}"
            self.expect(where, self.ty(c, params, where, spec=True, primed_ok=True), "Bool")
        for ctor, args in d.emits:
            where = f"emit {ctor}"
            for a in args:
                self.ty(a, params, where, spec=True, wild_ok=True)
        scope = dict(params)
        for s in d.body:
            if s[0] == "let":
                where = f"{s[1]} = {code(s[2])}"
                t = self.ty(s[2], scope, where, spec=False)
                if s[1] in scope or s[1] in HANDLERS or s[1] in self.defs:
                    self.err(where, f"`{s[1]}` shadows an existing name")
                if TYPES and t == "Unit":
                    self.err(where, "has no value (Unit)")
                scope[s[1]] = t
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
        return self.out


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


def check_module(defs):
    found = [p for d in defs.values() for p in problems(defs, d)] + cycles(defs)
    if found:
        raise WellFormedError(found)
    return defs

"""Infers what the agent never has to write: the effect set (`uses`) and the failure set (`fails`)."""
from __future__ import annotations

from .run import HANDLERS, effect_of, is_handler, state_op
from .syntax import flat, negate


def _walk(e):
    yield e
    if e[0] == "bin":
        yield from _walk(e[2])
        yield from _walk(e[3])
    elif e[0] == "app":
        for a in e[2]:
            yield from _walk(a)


def _stmt_exprs(s):
    if s[0] == "let":
        return [s[2]]
    if s[0] == "need":
        return [s[1], *s[3]]
    return [s[1]]                       # do, and the condition of an if


def called_names(d):
    out = []
    for s in flat(d.needs + d.body):
        for e in _stmt_exprs(s):
            for n in _walk(e):
                if n[0] == "app":
                    out.append(n[1][0])
                elif n[0] == "name":
                    out.append(n[1])
    return out


def uses(defs, d, seen=None):
    seen = (seen or set()) | {d.name}
    eff = set()
    for n in called_names(d):
        if is_handler(defs, n):
            eff.add(effect_of(defs, n))
        elif n in defs and n not in seen:
            eff |= uses(defs, defs[n], seen)
    for e in list(eff):                  # writing a store implies reading it
        if e.endswith(".write"):
            eff.discard(e[:-len("write")] + "read")
    return eff


def _arg_type(e, d):
    if e[0] == "int" or e[0] == "bin":
        return "Int"
    if e[0] == "name":
        t = dict(d.params).get(e[1])
        return t or "Int"
    return "Int"


def fails(defs, d, seen=None):
    seen = (seen or set()) | {d.name}
    out = {}
    for s in flat(d.needs + d.body):
        if s[0] == "need":
            out.setdefault(s[2], [_arg_type(a, d) for a in s[3]])
    for n in called_names(d):
        if n in defs and n not in seen:
            for k, v in fails(defs, defs[n], seen).items():
                out.setdefault(k, v)
    return out


WRITERS = ("ledger.debit", "ledger.credit")
ANY_ACCOUNT = "*"


def _account_param(e, d, lets):
    """Which parameter an Account expression denotes, or ANY_ACCOUNT if it cannot be named."""
    while e[0] == "name" and e[1] in lets:
        e = lets[e[1]]
    if e[0] == "name" and e[1] in dict(d.params):
        return e[1]
    return ANY_ACCOUNT


def writes(defs, d, seen=None):
    """The balances a call can change, as parameter names (ANY_ACCOUNT = could be any account).
    Writes only happen in body statements (the type checker forbids them inside expressions)."""
    seen = (seen or set()) | {d.name}
    lets, out = {}, set()
    for s in flat(d.body):              # either branch may run, so a write in any branch counts
        if s[0] == "let":
            lets[s[1]] = s[2]
        if s[0] != "do" or s[1][0] != "app":
            continue
        name, args = s[1][1][0], s[1][2]
        st = state_op(defs, name)
        if name in WRITERS:
            out.add(_account_param(args[0], d, lets))
        elif st and st[1] == "set":                  # state writes are "X.param" (or "X.*")
            out.add(f"{st[0]}.{_account_param(args[0], d, lets)}")
        elif name in defs and name not in seen:
            callee = defs[name]
            idx = {p: i for i, (p, _) in enumerate(callee.params)}
            for w in writes(defs, callee, seen):
                x, dot, p = w.rpartition(".")
                mapped = ANY_ACCOUNT if p == ANY_ACCOUNT else _account_param(args[idx[p]], d, lets)
                out.add(f"{x}.{mapped}" if dot else mapped)
    return out


def ledger_writes(ws):
    return {w for w in ws if "." not in w}


def state_writes(ws):
    """{state: {param or ANY_ACCOUNT}} from a writes() set."""
    out = {}
    for w in ws:
        if "." in w:
            x, _, p = w.rpartition(".")
            out.setdefault(x, set()).add(p)
    return out


def fmt_writes(ws, d):
    led = ledger_writes(ws)
    parts = ["any account"] if ANY_ACCOUNT in led else [p for p, _ in d.params if p in led]
    for x, ps in sorted(state_writes(ws).items()):
        parts += [f"{x}.{'*' if ANY_ACCOUNT in ps else p}" for p in (["*"] if ANY_ACCOUNT in ps else
                                                                   [q for q, _ in d.params if q in ps])]
    return " ".join(parts)


STATE_READS = ("ledger.balance", "ledger.total")      # owners are never written


def subst(e, m):
    """Replace names (parameters / lets) by expressions."""
    k = e[0]
    if k == "name":
        return m.get(e[1], e) if not e[2] else e
    if k == "bin":
        return ("bin", e[1], subst(e[2], m), subst(e[3], m))
    if k == "app":
        return ("app", e[1], [subst(a, m) for a in e[2]])
    return e


def _names(e):
    return {n[1] for n in _walk(e) if n[0] == "name"} | {n[1][0] for n in _walk(e) if n[0] == "app"}


def _reads_state(e):
    return any(n in STATE_READS or n.endswith(".get") for n in _names(e))


def lifted(defs, d, seen=None):
    """Conditions the call needs in order to succeed, stated in the caller's pre-state.

    Spec needs are already in the interface; this adds the *body-level* ones (own `need`s and,
    transitively, callees' needs), so callers and the prover can see when a call refuses. A
    condition is lifted only when it can be stated in the pre-state: it comes before any ledger
    write, or it does not read balances at all. `clock.now` is fine anywhere: it is the time of
    the whole top-level call, the same value in every state the call passes through.
    Returns [(cond, ctor, payload or None, origin)], origin = "" for d's own body."""
    seen = (seen or set()) | {d.name}
    out, lets, state = [], {}, {"clean": True}    # lets: name -> (expr in pre-state terms, usable)

    def pre_state(e):
        bad = [n for n in _names(e) if n in lets and not lets[n][1]]
        if bad:
            return None
        e2 = subst(e, {n: v for n, (v, _) in lets.items()})
        return e2 if state["clean"] or not _reads_state(e2) else None

    def add(cond, ctor, payload, origin, guards):
        c = pre_state(cond)
        if c is None or None in guards:
            return
        for g in reversed(guards):                # inside `if G`: the call refuses when G and not cond,
            ng = negate(g)                        # so callers may assume `not G or cond`
            if ng is None:
                return
            c = ("bin", "or", ng, c)
        p = [pre_state(a) for a in payload]
        item = (c, ctor, None if any(x is None for x in p) else p, origin)
        if all(code_key(item) != code_key(o) for o in out):
            out.append(item)

    def walk(stmts, guards):
        for s in stmts:
            if s[0] == "let":
                e = pre_state(s[2])
                lets[s[1]] = (e if e is not None else s[2], e is not None)
            elif s[0] == "need":
                add(s[1], s[2], s[3], "", guards)
            elif s[0] == "if":
                g = pre_state(s[1])
                ng = negate(g) if g is not None else None
                before = state["clean"]
                walk(s[2], guards + [g])
                after_then, state["clean"] = state["clean"], before
                walk(s[3], guards + [ng])
                state["clean"] = state["clean"] and after_then
            elif s[1][0] == "app":
                name, args = s[1][1][0], s[1][2]
                if name in defs and name not in seen:
                    callee = defs[name]
                    m = {p: a for (p, _), a in zip(callee.params, args)}
                    for n in callee.needs:
                        add(subst(n[1], m), n[2], [subst(a, m) for a in n[3]], name, guards)
                    for c, ctor, payload, origin in lifted(defs, callee, seen):
                        add(subst(c, m), ctor, [subst(a, m) for a in payload] if payload else [],
                            origin or name, guards)
                    if writes(defs, callee):
                        state["clean"] = False
                elif name in WRITERS or (state_op(defs, name) or ("", ""))[1] == "set":
                    state["clean"] = False

    walk(d.body, [])
    return out


def code_key(item):
    return repr(item[0]) + item[1]


def callees(defs, d):
    return [n for n in dict.fromkeys(called_names(d)) if n in defs and n != d.name]


def fmt_uses(eff):
    order = ["ledger.write", "ledger.read", "clock", "audit"]
    return " ".join(sorted(eff, key=lambda x: order.index(x) if x in order else 99))


def fmt_fails(fs):
    return " ".join(k + (f"({','.join(v)})" if v else "") for k, v in fs.items())

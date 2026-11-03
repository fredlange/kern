"""Infers what the agent never has to write: the effect set (`uses`) and the failure set (`fails`)."""
from __future__ import annotations

from .run import HANDLERS


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
    return [s[1]]


def called_names(d):
    out = []
    for s in d.needs + d.body:
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
        if n in HANDLERS:
            eff.add(HANDLERS[n][1])
        elif n in defs and n not in seen:
            eff |= uses(defs, defs[n], seen)
    if "ledger.write" in eff:
        eff.discard("ledger.read")
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
    for s in d.needs + d.body:
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
    for s in d.body:
        if s[0] == "let":
            lets[s[1]] = s[2]
        if s[0] != "do" or s[1][0] != "app":
            continue
        name, args = s[1][1][0], s[1][2]
        if name in WRITERS:
            out.add(_account_param(args[0], d, lets))
        elif name in defs and name not in seen:
            callee = defs[name]
            idx = {p: i for i, (p, _) in enumerate(callee.params)}
            for w in writes(defs, callee, seen):
                out.add(ANY_ACCOUNT if w == ANY_ACCOUNT else _account_param(args[idx[w]], d, lets))
    return out


def fmt_writes(ws, d):
    if ANY_ACCOUNT in ws:
        return "any account"
    return " ".join(p for p, _ in d.params if p in ws)


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
    return bool(_names(e) & set(STATE_READS))


def lifted(defs, d, seen=None):
    """Conditions the call needs in order to succeed, stated in the caller's pre-state.

    Spec needs are already in the interface; this adds the *body-level* ones (own `need`s and,
    transitively, callees' needs), so callers and the prover can see when a call refuses. A
    condition is lifted only when it can be stated in the pre-state: it comes before any ledger
    write, or it does not read balances at all; and it never involves `clock.now`.
    Returns [(cond, ctor, payload or None, origin)], origin = "" for d's own body."""
    seen = (seen or set()) | {d.name}
    out, lets, clean = [], {}, True               # lets: name -> (expr in pre-state terms, usable)

    def pre_state(e):
        bad = [n for n in _names(e) if n in lets and not lets[n][1]]
        if bad or "clock.now" in _names(e):
            return None
        e2 = subst(e, {n: v for n, (v, _) in lets.items()})
        return e2 if clean or not _reads_state(e2) else None

    def add(cond, ctor, payload, origin):
        c = pre_state(cond)
        if c is None:
            return
        p = [pre_state(a) for a in payload]
        item = (c, ctor, None if any(x is None for x in p) else p, origin)
        if all(code_key(item) != code_key(o) for o in out):
            out.append(item)

    for s in d.body:
        if s[0] == "let":
            e = pre_state(s[2])
            lets[s[1]] = (e if e is not None else s[2], e is not None)
        elif s[0] == "need":
            add(s[1], s[2], s[3], "")
        elif s[1][0] == "app":
            name, args = s[1][1][0], s[1][2]
            if name in defs and name not in seen:
                callee = defs[name]
                m = {p: a for (p, _), a in zip(callee.params, args)}
                for n in callee.needs:
                    add(subst(n[1], m), n[2], [subst(a, m) for a in n[3]], name)
                for c, ctor, payload, origin in lifted(defs, callee, seen):
                    add(subst(c, m), ctor, [subst(a, m) for a in payload] if payload else [], origin or name)
                if writes(defs, callee):
                    clean = False
            elif name in WRITERS:
                clean = False
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

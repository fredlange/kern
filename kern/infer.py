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


def callees(defs, d):
    return [n for n in dict.fromkeys(called_names(d)) if n in defs and n != d.name]


def fmt_uses(eff):
    order = ["ledger.write", "ledger.read", "clock", "audit"]
    return " ".join(sorted(eff, key=lambda x: order.index(x) if x in order else 99))


def fmt_fails(fs):
    return " ".join(k + (f"({','.join(v)})" if v else "") for k, v in fs.items())

"""What each reader gets. The agent's views are plain Kern text; token counts use a
Claude tokenizer when one is installed."""
from __future__ import annotations

import os

from .infer import fmt_fails, fmt_uses
from .syntax import code

try:
    import anthropic
    from tokenizers import Tokenizer

    _TOK = Tokenizer.from_file(os.path.join(os.path.dirname(anthropic.__file__), "tokenizer.json"))
except Exception:  # tokenizer is optional
    _TOK = None


def tokens(text):
    return len(_TOK.encode(text).ids) if _TOK else None


def _proof_line(r):
    if r["clean"]:
        return "  proof ok"
    bad = sum(g["status"] != "proven" for g in r["guarantees"])
    return f"  proof {bad} open"


def iface(r, why):
    """What a caller that only needs to *use* the definition reads."""
    d = r["def"]
    lines = [d.src_lines["header"][0], f"  uses {fmt_uses(r['uses'])}"]
    if r["fails"]:
        lines.append(f"  fails {fmt_fails(r['fails'])}")
    if why["count"]:
        lines.append(f"  why {why['count']}" + (" stale" if why["stale"] else ""))
    return "\n".join(lines)


def spec(r, why):
    """What a caller that must *reason about* the definition reads: interface + guarantees + proof status."""
    d = r["def"]
    lines = [iface(r, why)]
    lines += [f"  {l}" for l in d.src_lines["spec"] if not l.startswith("eg ")]
    lines.append(_proof_line(r))
    return "\n".join(lines)


def editor(reports, name, whys):
    """What an agent *editing* the definition reads: its full source + the spec view of each callee."""
    r = reports[name]
    d = r["def"]
    own = [iface(r, whys[name])] + [f"  {l}" for l in d.src_lines["spec"]] + ["body"] + \
          [f"  {l}" for l in d.src_lines["body"]]
    deps = [spec(reports[c], whys[c]) for c in r["callees"]]
    return "\n".join(own) + ("\n\n" + "\n\n".join(deps) if deps else "")


# ---------- English rendering for the human proof page (templates, no model) ----------

NAMES = {"ledger.balance": "balance of {0}", "ledger.owner": "owner of {0}", "ledger.total": "total money in the ledger"}
OPS = {"=": "=", "!=": "≠", ">=": "≥", "<=": "≤", ">": ">", "<": "<", "+": "+", "-": "−", "and": "and", "or": "or"}
NEG = {"=": "!=", "!=": "=", ">=": "<", "<=": ">", ">": "<=", "<": ">="}
EFFECTS = {"ledger.write": "the ledger (read and write)", "ledger.read": "the ledger (read only)",
           "clock": "the clock", "audit": "the audit log"}


def _has_prime(e):
    if e[0] == "name":
        return e[2]
    if e[0] == "app":
        return e[1][1] or any(_has_prime(a) for a in e[2])
    if e[0] == "bin":
        return _has_prime(e[2]) or _has_prime(e[3])
    return False


def en(e, lets=None, tense=False):
    lets = lets or {}
    k = e[0]
    if k == "int":
        return str(e[1])
    if k == "name":
        if e[1] in lets:
            return en(lets[e[1]], lets, tense)
        if e[1] in NAMES:
            return NAMES[e[1]] + (" after" if e[2] else " before" if tense else "")
        return f"<code>{e[1]}</code>"
    if k == "app":
        (n, primed), args = e[1], e[2]
        if n in NAMES:
            s = NAMES[n].format(*[en(a, lets, tense) for a in args])
            return s + (" <em>after</em>" if primed else " <em>before</em>" if tense else "")
        return f"<code>{n} {' '.join(en(a, lets, tense) for a in args)}</code>"
    if k == "bin":
        return f"{en(e[2], lets, tense)} {OPS[e[1]]} {en(e[3], lets, tense)}"


def en_ensure(e):
    return en(e, tense=_has_prime(e))


def en_refusal(need, lets):
    cond = need[1]
    if cond[0] == "bin" and cond[1] in NEG:
        neg = ("bin", NEG[cond[1]], cond[2], cond[3])
        text = en(neg, lets)
    else:
        text = f"not ({en(cond, lets)})"
    detail = f" ({', '.join(en(a, lets) for a in need[3])})" if need[3] else ""
    return text, f"{need[2]}{detail}"


def lets_of(d):
    return {s[1]: s[2] for s in d.body if s[0] == "let"}


def code_of(e):
    return code(e)

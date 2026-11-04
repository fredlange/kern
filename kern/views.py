"""What each reader gets. The agent's views are plain Kern text; token counts use a
Claude tokenizer when one is installed."""
from __future__ import annotations

import os

from .infer import fmt_fails, fmt_uses, fmt_writes
from .syntax import NEG, _side, atom_code, code, emit_code, flat, negate

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
    if r["writes"]:
        lines.append(f"  writes {fmt_writes(r['writes'], d)}")
    if r["fails"]:
        lines.append(f"  fails {fmt_fails(r['fails'])}")
    if why["count"]:
        lines.append(f"  why {why['count']}" + (" stale" if why["stale"] else ""))
    return "\n".join(lines)


def spec(r, why):
    """What a caller that must *reason about* the definition reads: interface + guarantees + proof status."""
    d = r["def"]
    lines = [iface(r, why)] + [f"  {l}" for l in spec_lines(d)]
    lines += [f"  {l}" for l in body_needs(r)]
    lines.append(_proof_line(r))
    return "\n".join(lines)


def spec_lines(d):
    """The spec clauses a *reader* gets. A `need as X …` / `emit as X …` line is shown expanded:
    otherwise a caller could not tell when the call refuses (or what it emits) without loading X's
    spec as well. Definitions without references get their source lines unchanged."""
    src = [l for l in d.src_lines["spec"] if not l.startswith("eg ")]
    if not d.ref_origin:
        return src
    needs = [f"need {code(n[1])} else {' '.join([n[2]] + [atom_code(a) for a in n[3]])}" for n in d.needs]
    emits = [f"emit {emit_code(ev)}" for ev in d.emits]
    out, done = [], set()
    for l in src:
        kind = l.split(" ", 1)[0]
        if kind in ("need", "emit"):
            if kind not in done:
                out += needs if kind == "need" else emits
                done.add(kind)
            continue
        out.append(l)
    return out


def editor(reports, name, whys):
    """What an agent *editing* the definition reads: its full source + the spec view of each callee."""
    r = reports[name]
    d = r["def"]
    own = [iface(r, whys[name])] + [f"  {l}" for l in d.src_lines["spec"]] + ["body"] + \
          [f"  {l}" for l in d.src_lines["body"]]
    deps = [spec(reports[c], whys[c]) for c in r["callees"]]
    return "\n".join(own) + ("\n\n" + "\n\n".join(deps) if deps else "")


# ---------- English rendering for the human proof page (templates, no model) ----------

NAMES = {"ledger.balance": "balance of {0}", "ledger.owner": "owner of {0}", "ledger.total": "total money in the ledger",
         "clock.now": "the time of the call"}
OPS = {"=": "=", "!=": "≠", ">=": "≥", "<=": "≤", ">": "&gt;", "<": "&lt;", "+": "+", "-": "−", "*": "×", "/": "÷", "and": "and", "or": "or", "when": "when"}
EFFECTS = {"ledger.write": "the ledger (read and write)", "ledger.read": "the ledger (read only)",
           "clock": "the clock", "audit": "the audit log"}


def body_needs(r):
    """The definition's own body-level refusal conditions, in `need` syntax, marked `body`.

    Inherited ones (from callees) are left to the callee's own spec view: showing them here
    measured 176 vs 90 tokens for payout's spec view (docs/FIXES.md, step 8). The prover and the
    proof page use the full lifted set."""
    out = []
    for cond, ctor, payload, origin in r["lifted"]:
        if origin:
            continue
        fail = " ".join([ctor] + [atom_code(a) for a in (payload or [])])
        out.append(f"body need {code(cond)} else {fail}")
    return out


def unguaranteed_writes(r):
    """Written balances (and state entries, `hold.id`) that no `ensure` describes the after-value of."""
    d, said = r["def"], set()

    def walk(e, quantified=None):
        if e[0] == "all":                                   # `… for j from A to B` describes a range
            return walk(e[4], (e[1], e[2], e[3]))
        if e[0] == "app" and e[1] == ("ledger.balance", True) and e[2][0][0] == "name":
            said.add(e[2][0][1])
        if e[0] == "app" and e[1][1] and e[1][0].endswith(".get") and e[2][0][0] == "name":
            x, key = e[1][0][:-4], e[2][0][1]
            if quantified and key == quantified[0]:
                said.add(f"{x}.[{code(quantified[1])},{code(quantified[2])})")
            said.add(f"{x}.{key}")
        for sub in (e[2:4] if e[0] == "bin" else e[2] if e[0] == "app" else []):
            walk(sub, quantified)
    for c in d.ensures:
        walk(c)
    out = [p for p, _ in d.params if p in r["writes"] and p not in said]
    return out + sorted(w for w in r["writes"] if "." in w and w not in said)


def loop_event_note(r):
    """A definition that emits events inside a loop: the channel is unconstrained (no spec form for
    per-iteration events yet) and must say so instead of looking silent."""
    from .syntax import flat
    from .infer import uses
    defs, chans = r["defs"], set()
    for s in flat(r["def"].body):
        if s[0] != "for":
            continue
        for t in flat(s[5]):
            if t[0] == "do" and t[1][0] == "app":
                n = t[1][1][0]
                if n == "audit.emit" or n.endswith(".emit"):
                    chans.add(n.rsplit(".", 1)[0])
                elif n in defs:
                    chans |= {u for u in uses(defs, defs[n]) if u == "audit" or u in getattr(defs, "channels", [])}
    return sorted(chans)


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
    if k == "all":
        return f"for every <code>{e[1]}</code> from {en(e[2], lets)} to {en(e[3], lets)} (not included): " + \
            (en_ensure(e[4]) if tense else en(e[4], lets))
    if k == "name":
        if e[1] in lets:
            return en(lets[e[1]], lets, tense)
        if e[1] in NAMES:
            return NAMES[e[1]] + (" after" if e[2] else " before" if tense else "")
        return f"<code>{e[1]}</code>"
    if k == "app":
        (n, primed), args = e[1], e[2]
        if n.endswith(".get") and len(args) == 1:            # declared state: `hold of id`
            s = f"{n[:-4]} of {en(args[0], lets, tense)}"
            return s + (" <em>after</em>" if primed else " <em>before</em>" if tense else "")
        if n == "distinct":
            names = [en(a, lets, tense) for a in args]
            return f"{', '.join(names[:-1])} and {names[-1]} are all different"
        if n in NAMES:
            s = NAMES[n].format(*[en(a, lets, tense) for a in args])
            return s + (" <em>after</em>" if primed else " <em>before</em>" if tense else "")
        return f"<code>{n} {' '.join(en(a, lets, tense) for a in args)}</code>"
    if k == "bin":
        show = lambda x: en(x, lets, tense)
        return f"{_side(e[2], e[1], False, show)} {OPS[e[1]]} {_side(e[3], e[1], True, show)}"


def en_emits(events):
    """Template for an `emit` guarantee: the exact events appended to the audit log (or, for
    ("channel", name, events), sent on that channel)."""
    if isinstance(events, tuple) and events[0] == "channel":
        text = en_emits(events[2])
        return text.replace("appends", "sends", 1).replace("to the audit log", f"on channel <code>{events[1]}</code>", 1)
    items = []
    for ctor, args, *cond in events:
        shown = ", ".join("any value" if a == ("name", "_", False) else en(a) for a in args)
        items.append(f"<code>{ctor}</code>" + (f" ({shown})" if shown else "") +
                     (f" <em>only when</em> {en(cond[0])}" if cond else ""))
    if any(len(ev) > 2 for ev in events):
        return ("appends exactly these events to the audit log, in order, each one only when its "
                "condition holds before the call: " + ", then ".join(items))
    if len(items) == 1:
        return f"appends exactly one event to the audit log: {items[0]}"
    return f"appends exactly {len(items)} events to the audit log, in order: " + ", then ".join(items)


def en_ensure(e):
    if e[0] == "all":
        return en(e, tense=True)
    if e[0] == "bin" and e[1] == "when":          # `E when C`: C is about the before-state
        return f"<em>when</em> {en(e[3], tense=True)}: {en(e[2], tense=_has_prime(e[2]))}"
    return en(e, tense=_has_prime(e))


def en_refusal(need, lets):
    cond = need[1]
    neg = negate(cond)
    if cond[0] == "app" and cond[1][0] == "distinct":
        names = [en(a, lets) for a in cond[2]]
        text = f"two of {', '.join(names[:-1])} and {names[-1]} are the same"
    else:
        text = en(neg, lets) if neg else f"not ({en(cond, lets)})"
    detail = f" ({', '.join(en(a, lets) for a in need[3])})" if need[3] else ""
    return text, f"{need[2]}{detail}"


def lets_of(d):
    return {s[1]: s[2] for s in flat(d.body) if s[0] == "let"}


def guarded_needs(stmts, guards=()):
    """Body needs with the `if` conditions (or their negation, in `else`) under which they run."""
    for s in stmts:
        if s[0] == "need":
            yield s, list(guards)
        elif s[0] == "if":
            yield from guarded_needs(s[2], (*guards, ("if", s[1])))
            yield from guarded_needs(s[3], (*guards, ("else", s[1])))


def code_of(e):
    return code(e)

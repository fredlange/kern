"""Ratchet baseline: the spec should only get stronger.

`kern baseline FILE.k` records, per definition, its guarantee strength and its guarantee
clauses. `kern check` compares against it and flags a definition whose guarantee strength
fell or whose guarantees were removed. Default file: <module>.strength.

File format:
    transfer 9/13
      keep ledger.total
      ensure ledger.balance' from = ledger.balance from - amount
"""
from __future__ import annotations

import os
import re

HEAD = re.compile(r"^(\w+) (\d+)/(\d+)$")
V2 = "# kern baseline v2: guarantees, spec needs and own body-level refusals"


def clauses(r):
    return [f"{g['kind']} {g['clause']}" for g in r["guarantees"]]


def refusals(r):
    """What a caller relies on about *when* the call refuses: the spec needs and the definition's
    own body-level conditions (inferred, `body need …`). Callees' conditions belong to the callees."""
    from .syntax import stmt_code
    from .views import body_needs
    return [stmt_code(n) for n in r["def"].needs] + body_needs(r)


def write(path, reports):
    with open(path, "w") as f:
        f.write(V2 + "\n")
        for n, r in reports.items():
            m = r["mutation"]
            if not m:
                continue                      # only clean, measured definitions set a baseline
            f.write(f"{n} {m['guarantee_killed']}/{m['total']}\n" +
                    "".join(f"  {c}\n" for c in clauses(r) + refusals(r)))


def load(path):
    if not path or not os.path.exists(path):
        return {}
    return parse(open(path).read())


def parse(text):
    """A baseline from its text. Old files (no v2 header) recorded guarantees only, so their
    needs are not compared (that would flag every need as new)."""
    base, cur = {}, None
    lines = text.splitlines()
    v2 = bool(lines) and lines[0].strip() == V2
    for line in lines:
        if not line.strip() or line.startswith("#"):
            continue
        m = HEAD.match(line.strip())
        if m and not line[0].isspace():
            cur = base[m.group(1)] = {"killed": int(m.group(2)), "total": int(m.group(3)), "clauses": [],
                                      "refusals": [], "v2": v2}
        elif cur is not None:
            c = line.strip()
            (cur["refusals"] if c.startswith(("need ", "body need ")) else cur["clauses"]).append(c)
    return base


def compare(reports, base):
    """Attach r["regression"] (list of messages) to every report weaker than its baseline."""
    for n, r in reports.items():
        b = base.get(n)
        r["regression"] = []
        if not b:
            continue
        gone = [c for c in b["clauses"] if c not in clauses(r)]
        if gone:
            r["regression"].append("guarantees removed since the baseline: " + "; ".join(gone))
        if b.get("v2"):                       # when the call refuses is part of what callers rely on
            now = refusals(r)
            dropped = [c for c in b["refusals"] if c not in now]
            added = [c for c in now if c not in b["refusals"]]
            if dropped:
                r["regression"].append("spec changed: refusals removed or changed since the baseline (it may now accept what it refused): " + "; ".join(dropped))
            if added:
                r["regression"].append("spec changed: new refusals since the baseline (it may now refuse what it accepted): " + "; ".join(added))
        m = r["mutation"]
        if m and b["total"] and m["guarantee_killed"] * b["total"] < b["killed"] * m["total"]:
            r["regression"].append(f"guarantee strength fell from {b['killed']}/{b['total']} "
                                   f"to {m['guarantee_killed']}/{m['total']}")
        if not m and r["clean"] is False and b["total"]:
            r["regression"].append("was clean at the baseline, is not now")
    return reports

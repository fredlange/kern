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


def clauses(r):
    return [f"{g['kind']} {g['clause']}" for g in r["guarantees"]]


def write(path, reports):
    with open(path, "w") as f:
        for n, r in reports.items():
            m = r["mutation"]
            if not m:
                continue                      # only clean, measured definitions set a baseline
            f.write(f"{n} {m['guarantee_killed']}/{m['total']}\n" + "".join(f"  {c}\n" for c in clauses(r)))


def load(path):
    base, cur = {}, None
    if not path or not os.path.exists(path):
        return base
    for line in open(path):
        if not line.strip():
            continue
        m = HEAD.match(line.strip())
        if m and not line[0].isspace():
            cur = base[m.group(1)] = {"killed": int(m.group(2)), "total": int(m.group(3)), "clauses": []}
        elif cur is not None:
            cur["clauses"].append(line.strip())
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
        m = r["mutation"]
        if m and b["total"] and m["guarantee_killed"] * b["total"] < b["killed"] * m["total"]:
            r["regression"].append(f"guarantee strength fell from {b['killed']}/{b['total']} "
                                   f"to {m['guarantee_killed']}/{m['total']}")
        if not m and r["clean"] is False and b["total"]:
            r["regression"].append("was clean at the baseline, is not now")
    return reports

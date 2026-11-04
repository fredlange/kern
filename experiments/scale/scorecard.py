"""The scale scorecard (docs/research/SCALE.md): the same measurements for every size of the app.

    .venv/bin/python experiments/scale/scorecard.py examples/escrow_s3.k      (from the repo root)

Floors (must hold at every size) and trends (compared size to size) are defined in
docs/research/SCALE.md; this module only measures.
"""
from __future__ import annotations

import re
import statistics
import time

import os
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))
from kern import views  # noqa: E402
from kern.infer import callees  # noqa: E402
from kern.report import analyse  # noqa: E402
from kern.syntax import parse_module  # noqa: E402
from kern.wf import check_module  # noqa: E402

NO_WHY = {"count": 0, "stale": False, "entries": []}


def blocks(src):
    """Definition name -> its full source text (header, spec, body)."""
    return {m.group(1): m.group(0).strip() for m in re.finditer(r"(?ms)^([a-z]\w*) .*?(?=^[a-z]\w* |\Z)", src)}


def chain(defs, n, acc=None):
    """n and every definition it reaches, transitively (what reading without views costs)."""
    acc = [] if acc is None else acc
    if n not in acc:
        acc.append(n)
        for c in callees(defs, defs[n]):
            chain(defs, c, acc)
    return acc


def depth(defs, n):
    cs = callees(defs, defs[n])
    return 1 + max(depth(defs, c) for c in cs) if cs else 0


def score(path):
    src = open(path).read()
    defs = check_module(parse_module(src))
    t0 = time.perf_counter()
    reps = analyse(defs, mutation=True)
    secs = time.perf_counter() - t0
    tiers = {}
    for r in reps.values():
        for g in r["guarantees"]:
            tiers[g["status"]] = tiers.get(g["status"], 0) + 1
    muts = [r["mutation"] for r in reps.values() if r["mutation"]]
    killed, total = sum(m["guarantee_killed"] for m in muts), sum(m["total"] for m in muts)
    ex_killed = sum(m["killed"] for m in muts)
    whys = {n: NO_WHY for n in defs}
    text = blocks(src)
    reading = {}
    for n in defs:
        if callees(defs, defs[n]):
            ed = views.tokens(views.editor(reps, n, whys))
            full = views.tokens("\n\n".join(text[x] for x in chain(defs, n)))
            reading[n] = (depth(defs, n), ed, full, ed / full)
    deepest = max(reading.values(), key=lambda v: (v[0], v[3]), default=None)
    spec_tok = sum(views.tokens("\n".join(d.src_lines["header"] + [l for l in d.src_lines["spec"] if not l.startswith("eg ")]))
                   for d in defs.values())
    body_tok = sum(views.tokens("\n".join(d.src_lines["body"])) for d in defs.values())
    return {
        "file": path, "defs": len(defs), "clean": sum(r["clean"] for r in reps.values()),
        "guarantees": sum(len(r["guarantees"]) for r in reps.values()), "tiers": tiers,
        "unconfirmed": tiers.get("unconfirmed", 0), "complete": sum(d.complete for d in defs.values()),
        "strength": (killed, total), "together": (ex_killed, total),
        "reading_median": statistics.median(v[3] for v in reading.values()) if reading else None,
        "reading_deepest": deepest, "max_depth": max((v[0] for v in reading.values()), default=0),
        "secs": secs, "secs_per_def": secs / len(defs),
        "source_tokens": views.tokens(src), "spec_tokens": spec_tok, "body_tokens": body_tok,
        "not_clean": [n for n, r in reps.items() if not r["clean"]],
        "missed": {n: [d for d, _ in r["mutation"]["guarantee_missed"]] for n, r in reps.items()
                   if r["mutation"] and r["mutation"]["guarantee_missed"]},
        "by_def": reading,
    }


def show(s):
    k, t = s["strength"]
    ek, _ = s["together"]
    d = s["reading_deepest"]
    lines = [
        f"{s['file']}: {s['defs']} definitions, {s['source_tokens']} source tokens "
        f"(spec {s['spec_tokens']}, body {s['body_tokens']})",
        f"  clean {s['clean']}/{s['defs']}   guarantees {s['guarantees']} {s['tiers']}   unconfirmed {s['unconfirmed']}",
        f"  ok otherwise {s['complete']}/{s['defs']}",
        f"  guarantee strength {k}/{t} = {k / t:.0%}   with examples {ek}/{t} = {ek / t:.0%}" if t else "  no mutants",
        f"  reading (editor view / transitive source): median {s['reading_median']:.2f}; "
        f"deepest (depth {d[0]}) {d[1]}/{d[2]} = {d[3]:.2f}" if d else "  no calls",
        f"  reading by depth (median): " + "  ".join(
            f"{k}: {statistics.median(v[3] for v in s['by_def'].values() if v[0] == k):.2f}"
            f" (n={sum(1 for v in s['by_def'].values() if v[0] == k)})"
            for k in sorted({v[0] for v in s['by_def'].values()})),
        f"  check {s['secs']:.1f}s = {s['secs_per_def']:.2f}s per definition",
    ]
    if s["not_clean"]:
        lines.append(f"  NOT CLEAN: {', '.join(s['not_clean'])}")
    for n, ms in s["missed"].items():
        lines += [f"  missed by guarantees, {n}: {m}" for m in ms]
    return "\n".join(lines)


if __name__ == "__main__":
    print(show(score(sys.argv[1])))

"""C6 runner (frozen before measuring). Each configuration runs `check` (analyse with mutation
testing, as `kern check`) in its own process, for peak memory, with a timeout.

    .venv/bin/python challenge/c6/run.py          (from the repo root; writes results.json)
"""
import json
import os
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(os.path.dirname(HERE))
TIMEOUT = 900

CHILD = r"""
import json, resource, sys, time
sys.path.insert(0, sys.argv[1]); sys.path.insert(0, sys.argv[2])
from gen import gen
from kern.report import analyse
from kern.syntax import parse_module
from kern.wf import check_module
d, k, e, n, state = map(int, sys.argv[3:8])
src = gen(d, k, e, n, bool(state))
defs = check_module(parse_module(src))
t = time.perf_counter()
reps = analyse(defs, mutation=True)
secs = time.perf_counter() - t
mut = [r["mutation"] for r in reps.values() if r["mutation"]]
print(json.dumps({"secs": secs, "per_def": secs / len(defs), "defs": len(defs),
                  "clean": sum(r["clean"] for r in reps.values()),
                  "guarantees": sum(len(r["guarantees"]) for r in reps.values()),
                  "mutants": sum(m["total"] for m in mut),
                  "max_rss_mb": resource.getrusage(resource.RUSAGE_SELF).ru_maxrss / 2**20}))
"""


def measure(d, k, e, n, state=0):
    try:
        p = subprocess.run([sys.executable, "-c", CHILD, HERE, ROOT, *map(str, (d, k, e, n, state))],
                           capture_output=True, text=True, timeout=TIMEOUT)
    except subprocess.TimeoutExpired:
        return {"d": d, "k": k, "e": e, "n": n, "state": state, "timeout": TIMEOUT}
    if p.returncode:
        return {"d": d, "k": k, "e": e, "n": n, "state": state, "error": p.stderr[-800:]}
    return dict({"d": d, "k": k, "e": e, "n": n, "state": state}, **json.loads(p.stdout))


PLAN = (
    [("P1", (6, 4, 6, 10, 0))] +
    [("P2", (3, 2, 3, 200, 0))] +
    [("d", (d, 2, 3, d, 0)) for d in range(1, 9)] +
    [("k", (3, k, 3, 3, 0)) for k in range(0, 7)] +
    [("e", (3, 2, e, 3, 0)) for e in range(0, 9)] +
    [("n", (3, 2, 3, n, 0)) for n in (10, 50)] +
    [("state", (3, 2, 3, 10, s)) for s in (0, 1)]
)


def reprove_check():
    """Does check re-prove an unchanged definition? Time a full check, then change one leaf and
    check again: with no cache the second run costs the same (expected: a gap)."""
    sys.path.insert(0, HERE)
    sys.path.insert(0, ROOT)
    import time
    from gen import gen
    from kern.report import analyse
    from kern.syntax import parse_module
    src = gen(3, 2, 3, 30)
    t = time.perf_counter(); analyse(parse_module(src), mutation=True); full = time.perf_counter() - t
    src2 = src.replace("eg A:500@u1 B:0@u2 | f0_1 A B 100", "eg A:600@u1 B:0@u2 | f0_1 A B 100", 1)
    t = time.perf_counter(); analyse(parse_module(src2), mutation=True); again = time.perf_counter() - t
    return {"n": 30, "full_secs": full, "after_one_change_secs": again, "incremental": again < 0.5 * full}


def main():
    rows = []
    for tag, cfg in PLAN:
        r = dict(measure(*cfg), tag=tag)
        rows.append(r)
        print(json.dumps(r), flush=True)
    rp = reprove_check()
    print(json.dumps(rp))
    p1 = next(r for r in rows if r["tag"] == "P1")
    p2 = next(r for r in rows if r["tag"] == "P2")
    verdict = {"P1_pass": "per_def" in p1 and p1["per_def"] <= 1.0 and p1["clean"] == p1["defs"],
               "P2_pass": "secs" in p2 and p2["secs"] <= 60 and p2["clean"] == p2["defs"]}
    json.dump({"rows": rows, "reprove": rp, "verdict": verdict}, open(f"{HERE}/results.json", "w"), indent=1)
    print(json.dumps(verdict))


if __name__ == "__main__":
    main()

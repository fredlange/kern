"""The S2 results digest (docs/research/ROADMAP_RUN.md, S2 result 1): SHA-256 over every clean flag, guarantee status,
counterexample-confirmed flag and mutation score of 10 modules, with full mutation testing and no cache.
Recovered verbatim from the overnight run on 2026-11-04 and saved here so the floor can be re-checked.

    .venv/bin/python validation/digest.py            (the S2 digest, 10 modules with full mutation testing; ~minutes)
    .venv/bin/python validation/digest.py --loops    (the loop/handler/package digest; ~1 minute)
"""
import hashlib
import json
import os
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, ROOT)
from validation.chains import gen  # noqa: E402  (the C6 generator; experiments/challenge/c6/gen.py is the frozen original)

from kern.report import analyse  # noqa: E402
from kern.syntax import parse_module  # noqa: E402

MODULES = ["escrow_s1", "escrow_s2", "escrow_s3", "ledger_v3", "ledger_v4", "ledger_v5", "ledger_v6", "ledger_v7",
           "ledger_large"]


LOOPS = ["examples/batch.k", "examples/raffle.k"] + [f"tests/probes/p{i}.k" for i in range(1, 8)] + \
    ["tests/probes/fz1.k", "tests/probes/fz2.k", "examples/shop", "examples/library", "examples/lottery"]


def digest_loops():
    """The floor for loop, handler, vacuity and package changes, which the S2 modules never exercise (no loops,
    no handlers): every guarantee status and counterexample-confirmed flag, and clean flags. No mutation testing
    (the probes are wrong on purpose). Recorded 2026-11-04 after FZ findings 1-7 and P."""
    from kern import package
    out = {}
    for f in LOOPS:
        r = analyse(package.load(os.path.join(ROOT, f)), mutation=False)
        out[f] = {n: (x["clean"], [(g["kind"], g["status"], (g.get("counterexample") or {}).get("confirmed_by_interpreter"))
                                   for g in x["guarantees"]]) for n, x in r.items()}
    return hashlib.sha256(json.dumps(out, sort_keys=True).encode()).hexdigest()[:16], out


def digest():
    srcs = {f: open(os.path.join(ROOT, "examples", f + ".k")).read() for f in MODULES}
    srcs["c6_small"] = gen(4, 3, 4, 8)
    out = {}
    for name, src in srcs.items():
        r = analyse(parse_module(src))
        out[name] = {n: (x["clean"], [(g["kind"], g["status"], (g.get("counterexample") or {}).get("confirmed_by_interpreter"))
                                      for g in x["guarantees"]],
                         x["mutation"] and (x["mutation"]["guarantee_killed"], x["mutation"]["total"], x["mutation"]["killed"]))
                     for n, x in r.items()}
    return hashlib.sha256(json.dumps(out, sort_keys=True).encode()).hexdigest()[:16], out


if __name__ == "__main__":
    h, out = digest_loops() if "--loops" in sys.argv else digest()
    if "--dump" in sys.argv:
        json.dump(out, open(sys.argv[sys.argv.index("--dump") + 1], "w"), indent=1, sort_keys=True)
    print(h)

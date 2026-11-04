"""Differential test: Kern interpreter vs the TS and Go translations of escrow_s1, on random cases.
Run from the repo root: .venv/bin/python experiments/compare/diff.py [SEED] [N]  (needs node >= 23 and go)"""
import sys as _sys, os as _os
_sys.path.insert(0, _os.path.dirname(_os.path.dirname(_os.path.abspath(__file__))))
import json
import os
import random
import subprocess
import sys

from kern.run import Interp, World
from kern.syntax import parse_module
from kern.wf import check_module

HERE = os.path.dirname(os.path.abspath(__file__))
defs = check_module(parse_module(open("examples/escrow/escrow_s1.k").read()))
interp = Interp(defs)
rng = random.Random(int(sys.argv[1]) if len(sys.argv) > 1 else 1)
ACC = ["A", "B", "E", "H"]
INTS = [-1, 0, 1, 5, 10, 50, 99, 100, 101, 150, 200, 300, 600]

cases, kern = [], []
for i in range(int(sys.argv[2]) if len(sys.argv) > 2 else 5000):
    d = rng.choice(list(defs.values()))
    world = {a: rng.choice([0, 0, 5, 50, 100, 200, 300, 600]) for a in ACC}
    args = [rng.choice(ACC) if t == "Account" else rng.choice(INTS) for _, t in d.params]
    cases.append({"def": d.name, "args": args, "world": world, "now": i})
    w = World(dict(world), {a: "u" for a in ACC}, i, [], {})
    outcome, w = interp.run(d.name, args, w)
    kern.append({"outcome": ["ok"] if outcome == ("ok",) else [outcome[0], list(outcome[1])],
                 "bal": w.bal, "events": [list(e) for e in w.audit]})

subprocess.run(["go", "build", "-o", "diffbin", "./cmd/diff"], cwd=f"{HERE}/go", check=True)
inp = "\n".join(json.dumps(c) for c in cases) + "\n"
impls = {
    "ts": subprocess.run(["node", "diff.ts"], input=inp, capture_output=True, text=True, cwd=f"{HERE}/ts"),
    "go": subprocess.run(["./diffbin"], input=inp, capture_output=True, text=True, cwd=f"{HERE}/go"),
}
outcomes = {}
for k in kern:
    outcomes[k["outcome"][0]] = outcomes.get(k["outcome"][0], 0) + 1
print("cases", len(cases), "kern outcomes", dict(sorted(outcomes.items())))
for name, p in impls.items():
    if p.returncode:
        print(name, "crashed:", p.stderr[-2000:])
        continue
    got = [json.loads(l) for l in p.stdout.splitlines()]
    bad = [(c, k, g) for c, k, g in zip(cases, kern, got) if k != g]
    print(f"{name}: {len(cases) - len(bad)}/{len(cases)} identical (outcome, balances, events)")
    for c, k, g in bad[:3]:
        print("  case", c, "\n   kern", k, "\n   ", name, g)

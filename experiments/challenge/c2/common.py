"""Shared by make_hidden.py and grade.py (frozen with them): cases, runners, applying edits."""
import json
import os
import random
import shutil
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
sys.path.insert(0, ROOT)
from kern.run import Interp, KernError, World  # noqa: E402
from kern.syntax import parse_module  # noqa: E402
from kern.wf import check_module  # noqa: E402

ACC = ["A", "B", "E", "H"]
INTS = [-1, 0, 1, 5, 10, 49, 50, 51, 60, 99, 100, 101, 150, 200, 300, 600, 999, 1000, 1001, 1200, 2000]


def random_cases(defs, n, seed, names=None):
    rng = random.Random(seed)
    pool = [d for d in defs.values() if names is None or d.name in names]
    out = []
    for i in range(n):
        d = rng.choice(pool)
        world = {a: rng.choice([0, 0, 5, 50, 100, 200, 600, 1500, 3000]) for a in ACC}
        args = [rng.choice(ACC) if t == "Account" else rng.choice(INTS) for _, t in d.params]
        out.append({"def": d.name, "args": args, "world": world, "now": i})
    return out


def run_kern(defs, cases):
    interp, out = Interp(defs), []
    for c in cases:
        if c["def"] not in defs:
            out.append({"outcome": ["missing"], "bal": {}, "events": []})
            continue
        w = World(dict(c["world"]), {a: "u" for a in ACC}, c["now"], [], {})
        try:
            outcome, w = interp.run(c["def"], c["args"], w)
        except KernError as e:
            out.append({"outcome": ["error", str(e)[:80]], "bal": {}, "events": []})
            continue
        out.append({"outcome": ["ok"] if outcome == ("ok",) else [outcome[0], list(outcome[1])],
                    "bal": w.bal, "events": [list(e) for e in w.audit]})
    return out


def run_ts(ts_dir, cases):
    shutil.copy(os.path.join(os.path.dirname(os.path.abspath(__file__)), "harness.ts"), f"{ts_dir}/c2_harness.ts")
    inp = "\n".join(json.dumps(c) for c in cases) + "\n"
    p = subprocess.run(["node", "c2_harness.ts"], input=inp, capture_output=True, text=True, cwd=ts_dir)
    if p.returncode:
        return None, p.stderr[-800:]
    lines = p.stdout.splitlines()
    return [json.loads(l) for l in lines], None


def apply(path, edits):
    s = open(path).read()
    for old, new in edits:
        if old == "":
            s = s.rstrip("\n") + "\n" + new
            continue
        if s.count(old) != 1:
            raise ValueError(f"{os.path.basename(path)}: edit anchor occurs {s.count(old)} times")
        s = s.replace(old, new)
    open(path, "w").write(s)


def workspace(tmp, kern_edits=None, ts_edits=None):
    """A copy of the original module (Kern and TS) with optional edits applied."""
    shutil.copy(f"{ROOT}/examples/escrow_s1.k", f"{tmp}/escrow_s1.k")
    shutil.copytree(f"{ROOT}/compare/ts", f"{tmp}/ts")
    if kern_edits:
        apply(f"{tmp}/escrow_s1.k", kern_edits)
    if ts_edits:
        apply(f"{tmp}/ts/escrow.ts", ts_edits)
    return tmp


def load_kern(path):
    return check_module(parse_module(open(path).read()))

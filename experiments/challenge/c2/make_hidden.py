"""Build the hidden acceptance cases (challenge/c2/hidden/T*.json) from the reference solutions,
and check that the Kern and TypeScript references agree on every case. Frozen with grade.py."""
import hashlib
import json
import os
import random
import sys
import tempfile

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from common import ACC, INTS, load_kern, random_cases, run_kern, run_ts, workspace  # noqa: E402

HERE = os.path.dirname(os.path.abspath(__file__))
REF = json.load(open(f"{HERE}/hidden/reference/solutions.json"))


def targeted(task, rng):
    out = []
    for i in range(300):
        world = {a: rng.choice([0, 50, 200, 1500, 3000]) for a in ACC}
        if task == "T1":
            amount = rng.choice([1, 50, 100, 150, 600, 1200])
            part = rng.choice([0, 1, amount // 2, amount - 1, amount, amount + 1])
            fee = rng.choice([-1, 0, 5, 50, part, part + 1])
            accs = rng.sample(ACC, 4) if rng.random() < 0.75 else [rng.choice(ACC) for _ in range(4)]
            out.append({"def": "settle", "args": accs + [amount, part, fee],
                        "world": world, "now": i})
        elif task == "T2":
            d = rng.choice(["capture", "purchase", "refund"])
            amount, fee = rng.choice([999, 1000, 1001, 1200, 2000]), rng.choice([0, 49, 50, 51, 60, 900])
            accs = rng.sample(ACC, 4)
            args = {"capture": accs[:3] + [amount, fee], "purchase": accs + [amount, fee],
                    "refund": accs + [amount, rng.choice([1, 100, amount - 1000 if amount > 1000 else 1]), fee]}[d]
            out.append({"def": d, "args": args, "world": world, "now": i})
        else:
            d = rng.choice(["move", "tip"])
            a = rng.choice(ACC)
            b = a if rng.random() < 0.6 else rng.choice(ACC)
            out.append({"def": d, "args": [a, b, rng.choice([-1, 0, 0, 1, 50])], "world": world, "now": i})
    return out


def main():
    hashes = {}
    for task in ("T1", "T2", "T3"):
        with tempfile.TemporaryDirectory() as tmp:
            workspace(tmp, REF[task]["kern"], REF[task]["ts"])
            defs = load_kern(f"{tmp}/escrow_s1.k")
            cases = targeted(task, random.Random({"T1": 11, "T2": 22, "T3": 33}[task])) + random_cases(defs, 300, seed=101)
            expected = run_kern(defs, cases)
            ts, err = run_ts(f"{tmp}/ts", cases)
            assert err is None, err
            bad = [i for i, (k, t) in enumerate(zip(expected, ts)) if k != t]
            assert not bad, f"{task}: references disagree on {len(bad)} cases, e.g. {cases[bad[0]]}"
        path = f"{HERE}/hidden/{task}.json"
        json.dump({"task": task, "cases": cases, "expected": expected}, open(path, "w"))
        hashes[task] = hashlib.sha256(open(path, "rb").read()).hexdigest()
        ok = sum(e["outcome"] == ["ok"] for e in expected)
        print(task, len(cases), "cases,", ok, "ok; Kern and TS references agree on all")
    json.dump(hashes, open(f"{HERE}/HIDDEN.sha256.json", "w"), indent=1)


if __name__ == "__main__":
    main()

"""Writes toolchain/tests/pyrandom_vectors.json: interleaved call sequences from CPython's random.Random, for the Rust port.
Run with the Python the toolchain's oracle uses (the repo's .venv, 3.9.6); the Rust test is toolchain/tests/pyrandom_parity.rs."""
import json, os, random, sys
SEEDS = [0, 1, 7, -3, 1_000_004, 2**31, 2**32, 2**40 + 5, 2**70 + 3, -(2**65), "7:hold", "1:sub_amount", "", "ünicode ✓", "x" * 300]
def ops(r, plan):
    out = []
    for i in range(400):
        k = plan.randrange(9)
        if k == 0: out.append(["randint", -5, 500, r.randint(-5, 500)])
        elif k == 1: out.append(["randint", 0, 0, r.randint(0, 0)])
        elif k == 2: out.append(["randrange", 3, 1000, r.randrange(3, 1000)])
        elif k == 3: out.append(["randrange", -10**12, 10**12, r.randrange(-10**12, 10**12)])
        elif k == 4: out.append(["random", r.random()])
        elif k == 5:
            seq = list(range(plan.randrange(1, 9))); out.append(["choice", len(seq), r.choice(seq)])
        elif k == 6:
            x = list(range(plan.randrange(0, 12))); r.shuffle(x); out.append(["shuffle", len(x), x])
        elif k == 7:
            b = plan.choice([0, 1, 31, 32, 33, 63, 64, 65, 100, 128]); out.append(["getrandbits", b, str(r.getrandbits(b))])
        else: out.append(["randint", -(2**62), 2**62, r.randint(-(2**62), 2**62)])
    return out
plan = random.Random(12345)
cases = [{"seed": s if isinstance(s, str) else str(s), "kind": "str" if isinstance(s, str) else "int", "ops": ops(random.Random(s), plan)} for s in SEEDS]
json.dump({"python": sys.version.split()[0], "cases": cases}, open(os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "toolchain", "tests", "pyrandom_vectors.json"), "w"))
print(sys.version.split()[0], sum(len(c["ops"]) for c in cases), "calls")

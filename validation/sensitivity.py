"""Does validation/difftest.py notice a wrong runtime? Plant one known bug at a time in kern-rt or in
the generator, run the differential test, expect it to fail; always restore the file.

    python validation/sensitivity.py [--per-def N]

A planted bug the test does not catch is reported as MISSED: a hole in the test's coverage, not a
pass.
"""
import argparse
import contextlib
import importlib
import io
import os
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, ROOT)
from validation import difftest  # noqa: E402

LIB = os.path.join(ROOT, "runtime", "src", "lib.rs")
GEN = os.path.join(ROOT, "kern", "emit_rust.py")

PLANTS = [
    ("refusal does not tick the clock", LIB, "if !matches!(out, Err(Stop::Fault(_))) {", "if out.is_ok() {"),
    ("refusal keeps its events", LIB, "            self.events.clear();\n        }\n        if !matches", "        }\n        if !matches"),
    ("division truncates toward zero", LIB, "a.div_euclid(b)", "a / b"),
    ("Int-map rollback keeps a new key", LIB, "w.imaps[m as usize].remove(&k);", "let _ = k;"),
    ("Account-map rollback skipped", LIB, "Undo::A(m, i, v) => w.amaps[m as usize][i as usize] = v,", "Undo::A(m, i, v) => { let _ = (m, i, v); }"),
    ("balance rollback skipped", LIB, "Undo::Bal(i, v) => w.bal[i as usize] = v,", "Undo::Bal(i, v) => { let _ = (i, v); }"),
    ("debit adds", LIB, "let v = sub(self.balance(a), n)?;", "let v = add(self.balance(a), n)?;"),
    ("total skips the last account", LIB, "self.w.bal.iter().try_fold", "self.w.bal[..self.w.bal.len().saturating_sub(1)].iter().try_fold"),
    ("`>=` compiled as `>`", GEN, "return f\"({l} {'==' if op == '=' else op} {r})\", \"Bool\"",
     "return f\"({l} {'==' if op == '=' else ('>' if op == '>=' else op)} {r})\", \"Bool\""),
    ("arguments hoisted right to left", GEN, "lets = \" \".join(f\"let {t} = {c};\" for t, c in zip(temps, codes))",
     "lets = \" \".join(reversed([f\"let {t} = {c};\" for t, c in zip(temps, codes)]))"),
    ("refusal payload uses the first argument twice", GEN, "payload = f\"({', '.join(c for c, _ in vals)})\" if vals else \"\"",
     "payload = f\"({', '.join(vals[0][0] for _ in vals)})\" if vals else \"\""),
    ("channel events emitted on audit", GEN, 'return self.hoist(f"tx.emit({self.channels.index(ch)}, {{}})", vals), "Unit"',
     'return self.hoist("tx.emit(0, {})", vals), "Unit"'),
    ("else branch dropped", GEN, "                if s[3]:\n                    out.append(f\"{ind}}} else {{\")",
     "                if False:\n                    out.append(f\"{ind}}} else {{\")"),
]


def run(per_def):
    importlib.reload(difftest.emit_rust)          # a planted generator bug must be the code that runs
    out = io.StringIO()
    with contextlib.redirect_stdout(out):
        rc = difftest.main(["--per-def", str(per_def)])
    total = [l for l in out.getvalue().splitlines() if l.startswith("TOTAL")]
    return rc, total[-1] if total else out.getvalue()[-500:]


def main(argv):
    ap = argparse.ArgumentParser()
    ap.add_argument("--per-def", type=int, default=100)
    a = ap.parse_args(argv)
    rc, total = run(a.per_def)
    print(f"{'clean':48} {'PASS' if rc == 0 else 'FAIL'}  {total}")
    if rc != 0:
        return 1
    missed = 0
    for what, path, old, new in PLANTS:
        src = open(path).read()
        if src.count(old) != 1:
            print(f"{what:48} PLANT DID NOT APPLY ({src.count(old)} matches)")
            missed += 1
            continue
        try:
            open(path, "w").write(src.replace(old, new))
            try:
                rc, total = run(a.per_def)
                verdict = "caught" if rc != 0 else "MISSED"
            except Exception as e:                    # e.g. the planted bug does not compile
                verdict, total = "caught (build)", str(e).splitlines()[0][:80]
        finally:
            open(path, "w").write(src)
        missed += verdict == "MISSED"
        print(f"{what:48} {verdict:14} {total}")
    print(f"missed {missed} of {len(PLANTS)}")
    return 1 if missed else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))

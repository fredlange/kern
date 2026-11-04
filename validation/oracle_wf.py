"""Well-formedness oracle for the Rust port: what kern/wf.py says about every module of the parser corpus
(validation/oracle_syntax.py), so toolchain/src/wf.rs can be checked against it.

    .venv/bin/python validation/oracle_wf.py OUT.json

Per parsed module: every problem check_module reports, in order (or the exception if wf crashes). Per definition
of a well-formed module: its first 40 body mutants (kern.check.mutants) with the body as JSON (AST tuples as
arrays) and problems() for the mutant, which is how mutation testing filters them.
"""
import dataclasses
import json
import os
import re
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, ROOT)

from kern.check import mutants  # noqa: E402
from kern.syntax import ParseError, parse_module, with_def  # noqa: E402
from kern.wf import WellFormedError, check_module, problems  # noqa: E402
from validation.oracle_syntax import corpus  # noqa: E402


SPEC_BAD = ["  ensure ledger.debit {p} 1 = 0", "  keep {p} > 0", "  need {p}' > 0 else X", "  ensure clock.now' = 0",
            "  ensure ({p} = 1 when 1 = 1) when 2 = 2", "  emit Ev _ (_ + 1)", "  ensure distinct {p}",
            "  need rand.below 3 > 0 else X", "  ensure {p} + {p} > 0", "  ensure {p} = ledger.owner {p}",
            "  keep ledger.debit {p} 1", "  emit Ev (ledger.credit {p} 1)", "  ensure {name} = 0", "  keep 1 = 1",
            "  ensure {p} > 0 for {p} from 0 to 3", "  need {p} * {p} > 0 else X", "  need {p} / 0 > 0 else X",
            "  ensure ledger.total' > ledger.total for j from {p} to 3", "  emit Ev {p} when {p}",
            "  need hold.get {p} > 0 else X", "  ensure audit.emit (Ev 1) = 0", "  need _ > 0 else X"]
BODY_BAD = ["  {p} = 1", "  ledger.balance {p}", "  {name}{args}", "  x9 = ledger.debit {p} 1", "  invariant 1 = 1",
            "  for j9 from 0 to 3\n    for j8 from 0 to 2\n      ledger.total", "  need {p} else X", "  _ = 1",
            "  y9 = 1\n  y9 = 2", "  if {p}\n    ledger.total", "  ledger.credit {p} (Ev 1)", "  z9 = Ev (ledger.debit {p} 1)",
            "  for {p} from 0 to 2\n    ledger.total", "  ledger.total = 1", "  w9 = {p}'", "  need 1 = 1 else Bad (audit.emit (Ev 1))",
            "  clock.now 3", "  {p} 4", "  result = 1", "  for j7 from {p} to (Ev 2)\n    invariant j7 > 0 when 1 = 1\n    ledger.total",
            "  v9 = distinct 1", "  u9 = distinct 1 {p}", "  t9 = 1 = 1 = 1"]


def variants(src):
    """Ill-formed variants of a well-formed module: one bad spec line or body line per definition, or a header with
    a parameter type flipped."""
    lines, out = src.splitlines(), []
    for i, line in enumerate(lines):
        m = re.match(r"^([a-z]\w*)((?:\s+\w+:\w+)*)\s*(?:->\s*(\w+))?\s*$", line)
        if not m or line.startswith(("state ", "channel ", "invariant ")):
            continue
        params = [p.split(":") for p in m.group(2).split()]
        p, name = (params[0][0] if params else "q"), m.group(1)
        args = "".join(f" {q}" for q, _ in params)
        for bad in SPEC_BAD:
            out.append(lines[:i + 1] + bad.format(p=p, name=name, args=args).split("\n") + lines[i + 1:])
        j = next((k for k in range(i + 1, len(lines)) if lines[k] == "body"), None)
        if j is not None:
            for bad in BODY_BAD:
                out.append(lines[:j + 1] + bad.format(p=p, name=name, args=args).split("\n") + lines[j + 1:])
        if params:
            flip = line.replace(":Int", ":TMP").replace(":Account", ":Int").replace(":TMP", ":Account")
            out.append(lines[:i] + [flip] + lines[i + 1:])
            out.append(lines[:i] + [line + " -> Int"] + lines[i + 1:])
            out.append(lines[:i] + [line + f" {p}:Int"] + lines[i + 1:])
    return ["\n".join(v) + "\n" for v in out]


def sources():
    out = list(corpus())
    base = [(n, s) for n, s in out if n.endswith(".k") or n.startswith("docs") or n.startswith("chains")]
    for name, src in base:
        try:
            check_module(parse_module(src))
        except Exception:  # noqa: BLE001
            continue
        out += [(f"{name}~wf{i}", v) for i, v in enumerate(variants(src))]
    return out


def main(argv):
    out, n_mut = [], 0
    for name, src in sources():
        try:
            defs = parse_module(src)
        except ParseError:
            continue
        rec = {"name": name, "src": src}
        try:
            check_module(defs)
            rec["problems"] = []
        except WellFormedError as e:
            rec["problems"] = e.problems
        except Exception as e:  # noqa: BLE001
            rec["crash"] = f"{type(e).__name__}: {e}"
        if rec.get("problems") == [] and "~wf" not in name:
            rec["mutants"] = []
            for d in defs.values():
                for i, (desc, body) in enumerate(mutants(d)):
                    if i >= 40:
                        break
                    md = dataclasses.replace(d, body=body, examples=[])
                    rec["mutants"].append({"def": d.name, "desc": desc, "body": json.loads(json.dumps(body)),
                                           "problems": problems(with_def(defs, md), md)})
                    n_mut += 1
        out.append(rec)
    with open(argv[0], "w") as f:
        json.dump(out, f, ensure_ascii=False)
    print(f"{len(out)} modules ({sum(bool(r.get('problems')) for r in out)} ill-formed, "
          f"{sum('crash' in r for r in out)} crashes), {n_mut} mutants -> {argv[0]}")


if __name__ == "__main__":
    main(sys.argv[1:])

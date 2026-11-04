"""Parser oracle for the Rust port (docs/research/RUST_PORT_PLAN.md): dump what kern/syntax.py makes of a corpus
of Kern sources, so toolchain/ can be checked byte for byte against it.

    .venv/bin/python validation/oracle_syntax.py OUT.json

Corpus: every .k under examples/ and tests/probes/, every ```kern block in docs/, synthetic chain modules
(validation/chains.py) and 200 fuzz programs (validation/fuzz.py, seed 1). Per source: either the ParseError text,
or the module declarations and, per definition, every field as JSON (AST tuples become arrays), code() of every
expression, stmt_code() of every flattened statement, hash and content_hash.
"""
import glob
import json
import os
import random
import re
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, ROOT)

from kern.syntax import ParseError, code, emit_code, flat, parse_module, stmt_code  # noqa: E402
from validation.chains import gen  # noqa: E402
from validation.fuzz import gen_handler, gen_loop  # noqa: E402


BAD = [
    "f a:Int\n  need a > 0\n", "f a:Int\n  need a > 0 else low\n", "f a:Int\n  need a > 0 else 3\n",
    "f a:Int\n  emit low a\n", "f a:Int\n  emit (a + 1)\n", "f a:Int\n  ok otherwise please\n",
    "f a:Int\n  ok otherwise\n  ok otherwise\n", "f a:Int\n  frob a\n", "  need a > 0 else X\n", "body\n",
    "F a:Int\n", "f a:Int\n  ensure a = 1 $\n", "f a:Int\n  ensure (a = 1\n", "f a:Int\n  ensure a = \n",
    "f a:Int\n  need a > 0 for j from 0 to 3 else X\n", "f a:Int\nbody\n  if a > 0\n", "f a:Int\nbody\n  else\n",
    "f a:Int\nbody\n  x = 1\n    y = 2\n", "f a:Int\nbody\n    x = 1\n  y = 2\n", "state s Bool\n",
    "invariant x:Int\n", "channel audit\n", "channel as\n", "handler rand n:Int\n", "handler rand.below n:Int\nbody\n",
    "f a:Int\n  need as g a\n", "g b:Int\n  need b > 0 else X\nf a:Int\n  need as g a a\n",
    "f a:Int\n  need as f a\n", "f a:Int\n  need as (a + 1)\n", "import money.ledger\n",
    "f a:Int\n  eg A:1@u | f -> ok extra | B:1 | C\n", "f a:Int\n  eg A:1 | f 1 -> ok\n",
    "f a:Int\n  eg A:1@u f 1 ok\n", "f a:Int\nbody\n  for k from 0 to a\n    x = k\n    invariant x >= 0\n",
    "f a:Int\nbody\n  for k from 0 to a\n", "f a:Int\n  emit Ev a when\n", "f a:Int\n  ensure a' = a when\n",
    "f a:Int -> \n", "f a:Int -> Int Int\n", "f a\n",
]


def corpus():
    out = []
    for path in sorted(glob.glob(os.path.join(ROOT, "examples/**/*.k"), recursive=True)
                       + glob.glob(os.path.join(ROOT, "tests/probes/*.k"))):
        out.append((os.path.relpath(path, ROOT), open(path).read()))
    for path in sorted(glob.glob(os.path.join(ROOT, "docs/**/*.md"), recursive=True)) + ["README.md", "AGENTS.md"]:
        text = open(os.path.join(ROOT, path)).read()
        for i, body in enumerate(re.findall(r"```kern[^\n]*\n(.*?)```", text, re.S)):
            out.append((f"{os.path.relpath(os.path.join(ROOT, path), ROOT)}#{i}", body))
    for d, k, e, n, state in [(1, 0, 0, 1, False), (2, 1, 1, 4, False), (3, 2, 2, 6, True), (4, 3, 1, 9, True)]:
        out.append((f"chains:{d},{k},{e},{n},{state}", gen(d, k, e, n, state)))
    for i, src in enumerate(BAD):
        out.append((f"bad#{i}", src))
    rnd = random.Random(1)
    files = [s for n, s in out if n.endswith(".k")]
    for i in range(300):                    # single-line perturbations of real files: the error paths
        lines = rnd.choice(files).splitlines()
        k = rnd.randrange(len(lines))
        op = rnd.choice(["drop", "dup", "dedent", "indent", "cut", "swap"])
        if op == "drop":
            del lines[k]
        elif op == "dup":
            lines.insert(k, lines[k])
        elif op == "dedent":
            lines[k] = lines[k].lstrip()
        elif op == "indent":
            lines[k] = "  " + lines[k]
        elif op == "cut" and lines[k]:
            c = rnd.randrange(len(lines[k]))
            lines[k] = lines[k][:c] + lines[k][c + 1:]
        elif op == "swap" and k + 1 < len(lines):
            lines[k], lines[k + 1] = lines[k + 1], lines[k]
        out.append((f"perturb#{i}:{op}", "\n".join(lines) + "\n"))
    for i in range(150):
        out.append((f"fuzz:loop#{i}", gen_loop(rnd)))
    for i in range(50):
        out.append((f"fuzz:handler#{i}", gen_handler(rnd)))
    return out


def j(x):
    """JSON-ready: tuples -> lists, dict keys kept (stringified only when not str)."""
    if isinstance(x, (list, tuple)):
        return [j(a) for a in x]
    if isinstance(x, dict):
        return [[j(k), j(v)] for k, v in x.items()]          # pairs: keys may be ints or tuples
    return x


def expr_codes(d):
    out = []
    for n in d.needs:
        out.append(code(n[1]))
    out += [code(e) for e in d.keeps] + [code(e) for e in d.ensures] + [emit_code(e) for e in d.emits]
    return out


def dump_def(d):
    return {
        "name": d.name, "params": j(d.params), "ret": d.ret, "needs": j(d.needs), "keeps": j(d.keeps),
        "ensures": j(d.ensures), "emits": j(d.emits), "complete": d.complete, "ref_origin": j(d.ref_origin),
        "emit_channel": j(d.emit_channel), "body": j(d.body), "src_lines": j(d.src_lines),
        "examples": [{"world": j(e.world), "call": j(e.call), "outcome": j(e.outcome), "after": j(e.after),
                      "src": e.src, "now": e.now, "maps": j(e.maps), "after_maps": j(e.after_maps)}
                     for e in d.examples],
        "assumed": getattr(d, "assumed", False),
        "codes": expr_codes(d), "stmts": [stmt_code(s) for s in flat(d.body)],
        "hash": d.hash, "content_hash": d.content_hash,
    }


def dump(src):
    try:
        defs = parse_module(src)
    except ParseError as e:
        return {"error": str(e)}
    return {"states": j(defs.states), "invariants": j(defs.invariants), "channels": defs.channels,
            "handlers": [dump_def(h) for h in defs.handlers.values()],
            "defs": [dump_def(d) for d in defs.values()]}


def main(argv):
    out = [{"name": name, "src": src, **dump(src)} for name, src in corpus()]
    with open(argv[0], "w") as f:
        json.dump(out, f, indent=0, ensure_ascii=False)
    print(f"{len(out)} sources, {sum(len(o.get('defs', [])) for o in out)} definitions, "
          f"{sum('error' in o for o in out)} parse errors -> {argv[0]}")


if __name__ == "__main__":
    main(sys.argv[1:])

"""Inference oracle for the Rust port: what kern/infer.py infers for every definition of every well-formed module of
the parser corpus (validation/oracle_syntax.py), so toolchain/src/infer.rs can be checked against it.

    .venv/bin/python validation/oracle_infer.py OUT.json

Per definition: called_names, uses (sorted), fails, writes (sorted), lifted (cond and payload as code, ctor,
origin), relevant_states, spec_functions, callees, and fmt_uses / fmt_fails / fmt_writes.
"""
import json
import os
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, ROOT)

from kern import infer  # noqa: E402
from kern.syntax import ParseError, code, parse_module  # noqa: E402
from kern.wf import check_module  # noqa: E402
from validation.oracle_syntax import corpus  # noqa: E402


def dump_def(defs, d):
    u, f, w = infer.uses(defs, d), infer.fails(defs, d), infer.writes(defs, d)
    return {
        "name": d.name, "called_names": infer.called_names(d), "uses": sorted(u), "fails": list(map(list, f.items())),
        "writes": sorted(w),
        "lifted": [[code(c), k, None if p is None else [code(a) for a in p], o] for c, k, p, o in infer.lifted(defs, d)],
        "relevant_states": list(map(list, infer.relevant_states(defs, d).items())),
        "spec_functions": infer.spec_functions(defs, d), "callees": infer.callees(defs, d),
        "fmt_uses": infer.fmt_uses(u), "fmt_fails": infer.fmt_fails(f), "fmt_writes": infer.fmt_writes(w, d),
    }


def main(argv):
    out = []
    for name, src in corpus():
        try:
            defs = check_module(parse_module(src))
        except ParseError:
            continue
        out.append({"name": name, "src": src, "defs": [dump_def(defs, d) for d in defs.values()]})
    with open(argv[0], "w") as f:
        json.dump(out, f, ensure_ascii=False)
    print(f"{len(out)} modules, {sum(len(m['defs']) for m in out)} definitions -> {argv[0]}")


if __name__ == "__main__":
    main(sys.argv[1:])

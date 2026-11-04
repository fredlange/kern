"""why oracle for the Rust port: kern/why.py's anchor_hash for every definition of every well-formed module of the
parser corpus, status() of every example .why file against every module next to it, and the loader's result (or
error) on hand-written why texts.

    .venv/bin/python validation/oracle_why.py OUT.json
"""
import glob
import json
import os
import sys
import tempfile

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, ROOT)

from kern import why  # noqa: E402
from kern.syntax import ParseError, parse_module  # noqa: E402
from kern.wf import check_module  # noqa: E402
from validation.oracle_syntax import corpus  # noqa: E402

TEXTS = ["transfer@0123abcd\n  a\n  b\n", "transfer@PLACEHOLDER\n  why\n", "transfer\n  why\n", "  orphan\nx@1\n",
         "a@1\n  x\nb@2\n\n  y\na@3\n  z\n", "a@1\r\n  x\r\n", "a@1\r  x\r", "a@1\n\tx must hold\n  never do it\n",
         "a@ 1\n", "a@1 2\n", "", "\n\n", "a@1\n  only once, Always\n  mustn't\n  ok\n"]


def main(argv):
    out = {"anchors": [], "status": [], "texts": []}
    for name, src in corpus():
        try:
            defs = check_module(parse_module(src))
        except ParseError:
            continue
        out["anchors"].append({"src": src, "anchors": [why.anchor_hash(defs, d) for d in defs.values()]})
    for wpath in sorted(glob.glob(os.path.join(ROOT, "examples/**/*.why"), recursive=True)):
        store = why.load(wpath)
        for kpath in sorted(glob.glob(os.path.join(os.path.dirname(wpath), "*.k"))):
            try:
                defs = check_module(parse_module(open(kpath).read()))
            except ParseError:
                continue
            out["status"].append({"why": open(wpath).read(), "src": open(kpath).read(),
                                  "status": {n: why.status(store, d, defs) for n, d in defs.items()},
                                  "orphans": why.orphans(store, defs)})
    for t in TEXTS:
        with tempfile.NamedTemporaryFile("w", suffix=".why", delete=False, newline="") as f:
            f.write(t)
        try:
            store = why.load(f.name)
            out["texts"].append({"text": t, "store": [[n, s["hash"], s["entries"]] for n, s in store.items()]})
        except why.WhyFileError as e:
            out["texts"].append({"text": t, "error": str(e).replace(f.name, "PATH")})
        os.unlink(f.name)
    with open(argv[0], "w") as f:
        json.dump(out, f, ensure_ascii=False)
    print(f"{len(out['anchors'])} modules, {len(out['status'])} why/module pairs, {len(out['texts'])} texts -> {argv[0]}")


if __name__ == "__main__":
    main(sys.argv[1:])

"""Package-loader oracle for the Rust port: what kern/package.py makes of every example package (from the directory
and from each of its files), every single .k file, and the error cases of tests/test_package.py rebuilt under a
scratch root, so toolchain/src/package.rs can be checked against it. Paths in the dump are relative to the root.

    .venv/bin/python validation/oracle_package.py OUT.json SCRATCH_DIR
"""
import glob
import json
import os
import shutil
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, ROOT)

from kern import package  # noqa: E402
from kern.syntax import ParseError  # noqa: E402

LEDGER = open(os.path.join(ROOT, "examples/shop/money/ledger.k")).read()
PAY = """import money.ledger

pay a:Account b:Account
  need as transfer a b 5
  ok otherwise
  keep ledger.total
  emit as transfer a b 5
  eg A:10@u1 B:0@u2 | pay A B -> ok | A:5 B:5
body
  transfer a b 5
"""  # as in toolchain/tests/package.rs, formerly read from tests/test_package.py (deleted in 33ca18f)
CAT = "state stock Int\n\nrestock item:Int\n  ok otherwise\nbody\n  stock.set item 1\n"
SELL = "sell item:Int\n  ok otherwise\nbody\n  stock.set item 0\n"
CASES = {   # name -> (files, is a package, file to load)
    "not_imported": ({"money/ledger.k": LEDGER, "pay.k": PAY.replace("import money.ledger\n", "")}, True, "pay.k"),
    "state_not_imported": ({"catalog.k": CAT, "sell.k": SELL}, True, "sell.k"),
    "unknown_module": ({"pay.k": PAY.replace("import money.ledger", "import money.ledgr")}, True, "pay.k"),
    "duplicate": ({"a.k": LEDGER, "b.k": LEDGER}, True, "a.k"),
    "dup_state": ({"a.k": CAT, "b.k": CAT.replace("restock", "refill")}, True, "a.k"),
    "loose": ({"money/ledger.k": LEDGER, "pay.k": PAY}, False, "pay.k"),
    "loose_nested": ({"money/ledger.k": LEDGER, "pay.k": PAY, "top.k": "import pay\n\n" + PAY.replace(
        "import money.ledger\n", "").replace("pay a", "top a").replace("  transfer a b 5", "  pay a b")
        .replace("need as transfer a b 5", "need as pay a b").replace("emit as transfer a b 5", "emit as pay a b")
        .replace("| pay A B", "| top A B")}, False, "top.k"),
    "parse_error": ({"a.k": LEDGER, "b.k": "f a:Int\n  frob\n"}, True, "a.k"),
    "wf_error": ({"a.k": LEDGER, "b.k": "f a:Int\nbody\n  g a\n"}, True, "a.k"),
    "hidden_dir": ({"a.k": LEDGER, ".git/x.k": "garbage here\n", "sub/.cache/y.k": "garbage\n"}, True, "a.k"),
    "as_loop": ({"a.k": "f a:Int\n  need as g a\nbody\n  g a\n", "b.k": "g a:Int\n  need as f a\nbody\n  ledger.total\n"},
                True, "a.k"),
    "crlf": ({"money/ledger.k": LEDGER.replace("\n", "\r\n"), "pay.k": PAY.replace("\n", "\r\n")}, True, "pay.k"),
}
for i, bad in enumerate(["import ../ledger", "import /etc/passwd", "import money..ledger", "import Money", "import",
                         "import  money.ledger", "import money.ledger   "]):
    CASES[f"bad_import{i}"] = ({"money/ledger.k": LEDGER, "pay.k": PAY.replace("import money.ledger", bad)}, True, "pay.k")


def dump(path, root):
    rel = lambda p: os.path.relpath(p, root) if p else p  # noqa: E731
    try:
        defs = package.load(path)
    except ParseError as e:
        return {"error": str(e).replace(root + os.sep, "ROOT/").replace(root, "ROOT")}
    out = {"defs": [[n, d.hash, getattr(d, "module", None), rel(getattr(d, "file", None)), getattr(d, "line", None),
                     [list(x) for x in d.ref_origin.items()] and [[list(k), v] for k, v in d.ref_origin.items()]]
                    for n, d in defs.items()],
           "handlers": [[n, getattr(h, "module", None), getattr(h, "line", None)] for n, h in defs.handlers.items()],
           "states": list(map(list, defs.states.items())), "channels": defs.channels,
           "invariants": [[v, t] for v, t, _ in defs.invariants]}
    pkg = getattr(defs, "package", None)
    if pkg:
        out["package"] = {"root": rel(pkg["root"]), "base": rel(pkg["base"]),
                          "files": [[m, rel(f)] for m, f in pkg["files"].items()],
                          "imports": list(map(list, pkg["imports"].items())),
                          "origin": [[k, list(map(list, v.items()))] for k, v in pkg["origin"].items()]}
    return out


def main(argv):
    out, scratch = [], os.path.abspath(argv[1])
    shutil.rmtree(scratch, ignore_errors=True)
    shutil.copytree(os.path.join(ROOT, "examples"), os.path.join(scratch, "examples"),
                    ignore=shutil.ignore_patterns(".kern-cache", "proof-pages", "web"))
    for d in sorted(glob.glob(os.path.join(scratch, "examples", "*"))):
        targets = ([d] if os.path.exists(os.path.join(d, "kern.pkg")) else []) + sorted(glob.glob(os.path.join(d, "**", "*.k"), recursive=True))
        for t in targets:
            out.append({"load": os.path.relpath(t, scratch), **dump(t, scratch)})
    for name, (files, pkg, entry) in CASES.items():
        base = os.path.join(scratch, "cases", name)
        os.makedirs(base)
        if pkg:
            open(os.path.join(base, "kern.pkg"), "w").close()
        for relp, src in files.items():
            os.makedirs(os.path.dirname(os.path.join(base, relp)), exist_ok=True)
            with open(os.path.join(base, relp), "w", newline="") as f:
                f.write(src)
        out.append({"load": os.path.relpath(os.path.join(base, entry), scratch), **dump(os.path.join(base, entry), scratch)})
    with open(argv[0], "w") as f:
        json.dump(out, f, ensure_ascii=False, indent=0)
    print(f"{len(out)} loads ({sum('error' in o for o in out)} errors) -> {argv[0]}")


if __name__ == "__main__":
    main(sys.argv[1:])

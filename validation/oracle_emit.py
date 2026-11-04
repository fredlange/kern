"""Emitter oracle for the Rust port: the crate kern/emit_rust.py generates for every module of the analyse corpus
(validation/oracle_analyse.py) and every fuzz program of the parser corpus, or the Unsupported/ParseError message,
so toolchain/src/emit_rust.rs can be checked byte for byte. The runtime path in Cargo.toml is written as RUNTIME.

    .venv/bin/python validation/oracle_emit.py OUT.json
"""
import json
import os
import sys
import tempfile

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, ROOT)

from kern import emit_rust  # noqa: E402
from kern.syntax import ParseError  # noqa: E402
from validation.oracle_analyse import corpus  # noqa: E402


EXTRA = {   # the Unsupported paths
    "ctor_types": "f a:Account n:Int\n  need n > 0 else Bad n\n  need n < 9 else Bad a\nbody\n  ledger.debit a 0\n",
    "event_types": "f a:Account n:Int\nbody\n  audit.emit (Ev n)\n  audit.emit (Ev a)\n",
    "nested_event": "f n:Int\nbody\n  audit.emit (Ev (Inner n))\n",
    "names": "f type:Int fn:Account tx:Int\n  need type > 0 else Low type\nbody\n  ledger.credit fn tx\n  audit.emit (Ev fn)\n",
}


def main(argv):
    out = []
    with tempfile.TemporaryDirectory() as tmp:
        extra = []
        for name, src in EXTRA.items():
            extra.append(os.path.join(tmp, name + ".k"))
            open(extra[-1], "w").write(src)
        for i, path in enumerate(corpus() + extra):
            rec = {"path": os.path.relpath(path, ROOT) if path.startswith(ROOT) else None, "src": open(path).read()
                   if not os.path.isdir(path) else None, "crate": emit_rust.crate_name(path)}
            try:
                d = emit_rust.emit(path, os.path.join(tmp, str(i)))
                rec["files"] = {rel: open(os.path.join(d, rel)).read().replace(emit_rust.RUNTIME, "RUNTIME")
                                for rel in ("Cargo.toml", "src/main.rs", "src/module.rs")}
            except emit_rust.Unsupported as e:
                rec["unsupported"] = str(e)
            except ParseError as e:
                rec["error"] = str(e)
            out.append(rec)
    with open(argv[0], "w") as f:
        json.dump(out, f, ensure_ascii=False)
    print(f"{len(out)} modules ({sum('files' in r for r in out)} emitted, {sum('unsupported' in r for r in out)} "
          f"unsupported) -> {argv[0]}")


if __name__ == "__main__":
    main(sys.argv[1:])

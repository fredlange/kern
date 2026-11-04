"""Prover oracle for the Rust port (docs/research/RUST_PORT_PLAN.md, R1): what kern/prove.py concludes for every
definition, before trust propagation, so toolchain/src/prove.rs can be checked against it on its own.

    .venv/bin/python validation/oracle_prove.py build/oracle_prove.json [--fuzz N]

Corpus: validation/oracle_analyse.py's (every example, probes, docs blocks, chains, fuzz programs). Each module is
proved in a fresh process: within one process, Z3's witness choice depends on the solver calls made before. Per
definition: Prover(defs).prove(d) as JSON (every guarantee with its counterexample, and relied_on), plus
prove(d, first_kill=True), the call mutation testing makes. toolchain/tests/prove_oracle.rs runs the Rust prover on
the same paths, one thread per module (the Rust Z3 context and fresh counter are per thread).
"""
import json
import os
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, ROOT)


def dump_one(path):
    from kern import package
    from kern.prove import Prover
    from kern.syntax import ParseError
    from validation.oracle_analyse import canon
    try:
        defs = package.load(path)
    except ParseError as e:
        return {"error": f"{type(e).__name__}: {e}"}
    out = {}
    for d in defs.values():
        g, relied = Prover(defs).prove(d)
        out[d.name] = {"guarantees": canon(g), "relied_on": relied}
    for d in defs.values():
        g, relied = Prover(defs).prove(d, first_kill=True)
        out[d.name]["first_kill"] = [x["status"] for x in g]
    return {"defs": out}


def main(argv):
    if argv and argv[0] == "--one":
        print(json.dumps(dump_one(argv[1]), ensure_ascii=False))
        return 0
    from validation.oracle_analyse import corpus
    fuzz = int(argv[argv.index("--fuzz") + 1]) if "--fuzz" in argv else 60
    out = []
    for p in corpus(fuzz):
        r = subprocess.run([sys.executable, os.path.abspath(__file__), "--one", p], capture_output=True, text=True,
                           env=dict(os.environ, PYTHONHASHSEED="0"))
        rec = json.loads(r.stdout) if r.returncode == 0 else {"error": f"python exited {r.returncode}: {r.stderr[-300:]}"}
        out.append({"path": os.path.relpath(p, ROOT), **rec})
    with open(argv[0], "w") as f:
        json.dump(out, f, ensure_ascii=False)
    n = sum(len(o.get("defs", {})) for o in out)
    print(f"{len(out)} modules, {n} definitions -> {argv[0]}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))

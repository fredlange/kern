"""Analysis oracle for the Rust port (docs/research/RUST_PORT_PLAN.md, R1): what the Python toolchain concludes
about each module, as JSON, and a diff of that against the Rust toolchain's answer for the same files.

    .venv/bin/python validation/oracle_analyse.py dump PATH [--mutation]      one module (a .k file or a package dir)
    .venv/bin/python validation/oracle_analyse.py corpus [--fuzz N]           list the corpus (materialised in build/oracle/)
    .venv/bin/python validation/oracle_analyse.py diff --rust BIN [--mutation] [--fuzz N] [--only SUBSTR]
    .venv/bin/python validation/oracle_analyse.py diff --rust self        the harness against itself (another hash seed)

The Rust side must answer `BIN oracle analyse PATH [--mutation]` with the same JSON on stdout.

Per module: the well-formedness error (or none), and per definition the inferred interface (uses, fails, writes,
lifted refusals, callees), every guarantee (kind, clause, status, blocked_by, vacuity, the counterexample), the
examples, the property tests, the mutation score (with --mutation), the clean flag, the three agent views, and the
terminal output of `check`.

Comparison tiers (agreed by kern-9b and kern-8b):
- **exact:** everything except counterexample witnesses. That includes statuses, trust tiers, claim text, kill
  counts, property-test counts and violations (MT19937 makes them reproducible), views and the terminal output.
- **witness:** a counterexample's `eg` line and `emitted` events may differ. The Rust witness must replay, in this
  (Python) interpreter, to exactly the outcome its eg line states, and `confirmed_by_interpreter` must agree. In a
  module with assumed handlers the witness must be identical: an eg line doesn't record the handler's draws.

Each side dumps each module in a fresh process: within one process, Z3's witness choice depends on the solver calls
made before (CLAUDE.md, known limitations), so a long-running dump would make witnesses depend on corpus order.
  Terminal and view lines that print a witness are compared with the witness replaced by `<eg>`.
"""
from __future__ import annotations

import argparse
import glob
import json
import os
import re
import shutil
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, ROOT)

from kern import package, render, views  # noqa: E402
from kern.report import analyse  # noqa: E402
from kern.syntax import ParseError  # noqa: E402

OUT = os.path.join(ROOT, "build", "oracle")
NO_WHY = {"count": 0, "stale": False, "entries": []}
SKIP = {"def", "defs", "cached"}            # objects, and the cache flag (a property of the run, not the module)


def canon(x):
    """JSON-ready and order-stable: sets sorted, tuples as lists, Defs by name, dict keys as strings."""
    if isinstance(x, (set, frozenset)):
        return sorted((canon(a) for a in x), key=lambda a: json.dumps(a, sort_keys=True))
    if isinstance(x, (list, tuple)):
        return [canon(a) for a in x]
    if isinstance(x, dict):
        return {str(k): canon(v) for k, v in x.items() if k not in SKIP}
    if hasattr(x, "name") and hasattr(x, "params"):
        return x.name
    if isinstance(x, (str, int, float, bool)) or x is None:
        return x
    return repr(x)


def dump(path, mutation=False):
    """The Python toolchain's conclusions about one module."""
    try:
        defs = package.load(path)
    except ParseError as e:                                  # parse and well-formedness errors (WellFormedError)
        return {"error": f"{type(e).__name__}: {e}"}
    reports = analyse(defs, mutation=mutation)
    whys = {n: NO_WHY for n in reports}
    out = {"defs": {}, "terminal": render.terminal(reports, whys)}
    for n, r in reports.items():
        out["defs"][n] = {**canon(r),
                          "views": {"iface": views.iface(r, NO_WHY), "spec": views.spec(r, NO_WHY),
                                    "editor": views.editor(reports, n, whys)}}
    return out


# ---- the corpus -------------------------------------------------------------------------------------------------

def corpus(fuzz=60):
    """Paths (files or package dirs): every example, every probe, every ```kern block in docs/, small chain modules
    and `fuzz` generated loop/handler programs. Generated sources are written under build/oracle/corpus/."""
    import random
    from validation.chains import gen
    from validation.fuzz import gen_handler, gen_loop
    paths = []
    for d in sorted(glob.glob(os.path.join(ROOT, "examples", "*", ""))):
        if os.path.exists(os.path.join(d, "kern.pkg")):
            paths.append(d.rstrip(os.sep))
        else:
            paths += sorted(glob.glob(os.path.join(d, "*.k")))
    paths += sorted(glob.glob(os.path.join(ROOT, "tests", "probes", "*.k")))
    gen_dir = os.path.join(OUT, "corpus")
    shutil.rmtree(gen_dir, ignore_errors=True)
    os.makedirs(gen_dir)
    generated = []
    for doc in sorted(glob.glob(os.path.join(ROOT, "docs", "**", "*.md"), recursive=True)):
        for i, body in enumerate(re.findall(r"```kern[^\n]*\n(.*?)```", open(doc).read(), re.S)):
            generated.append((f"doc_{os.path.basename(doc)[:-3]}_{i}", body))
    for d, k, e, n, state in [(1, 0, 0, 1, False), (2, 1, 1, 4, False), (3, 2, 2, 6, True)]:
        generated.append((f"chains_{d}{k}{e}{n}{int(state)}", gen(d, k, e, n, state)))
    rnd = random.Random(1)
    for i in range(fuzz):
        generated.append((f"fuzz_{i}", gen_loop(rnd) if i % 4 else gen_handler(rnd)))
    for name, src in generated:
        p = os.path.join(gen_dir, name + ".k")
        open(p, "w").write(src)
        paths.append(p)
    return paths


# ---- the diff ---------------------------------------------------------------------------------------------------

WITNESS = re.compile(r"eg [^\n]*")


def _mask(text):
    return WITNESS.sub("<eg>", text) if isinstance(text, str) else text


def compare(py, rs, defs_for_replay=None, where=""):
    """Differences between two dumps of one module, as human-readable lines (empty: they agree)."""
    out = []
    if "error" in py or "error" in rs:
        # Python's text starts with the exception's class name (ParseError, LoadError, WellFormedError), which
        # the Rust error type doesn't carry: compare the messages
        strip = lambda e: re.sub(r"^\w+: ", "", e) if isinstance(e, str) and not e.startswith("python exited") else e
        if strip(py.get("error")) != rs.get("error"):
            out.append(f"{where}error: python {py.get('error')!r} vs rust {rs.get('error')!r}")
        return out
    if _mask(py["terminal"]) != _mask(rs["terminal"]):
        out.append(f"{where}terminal output differs")
    if list(py["defs"]) != list(rs["defs"]):
        out.append(f"{where}definitions: python {list(py['defs'])} vs rust {list(rs['defs'])}")
        return out
    for n in py["defs"]:
        out += _walk(py["defs"][n], rs["defs"][n], f"{where}{n}.")
    return out


def _walk(a, b, at):
    if isinstance(a, dict) and isinstance(b, dict):
        if "eg" in a and "confirmed_by_interpreter" in a:            # a counterexample: the witness tier
            out = []
            if a["confirmed_by_interpreter"] != b.get("confirmed_by_interpreter"):
                out.append(f"{at}confirmed_by_interpreter: {a['confirmed_by_interpreter']} vs "
                           f"{b.get('confirmed_by_interpreter')}")
            if b.get("eg") != a["eg"] and _REPLAY_DEFS.get("handlers") and "draws" not in b:
                out.append(f"{at}eg: python {a['eg']!r} vs rust {b.get('eg')!r} (a module with assumed handlers: an eg "
                           f"line doesn't record the draws, so without the rust side's draws it must be identical)")
            elif b.get("eg") != a["eg"] and b.get("confirmed_by_interpreter"):
                ok, why = replay(b.get("eg", ""), b.get("draws"))
                if not ok:
                    out.append(f"{at}eg: the rust witness {b.get('eg')!r} does not replay ({why})")
            return out
        b = {k: v for k, v in b.items() if k != "draws"}
        if set(a) != set(b):
            return [f"{at}keys: python-only {sorted(set(a) - set(b))}, rust-only {sorted(set(b) - set(a))}"]
        return [d for k in a for d in _walk(a[k], b[k], f"{at}{k}.")]
    if isinstance(a, list) and isinstance(b, list):
        if len(a) != len(b):
            return [f"{at[:-1]}: {len(a)} items vs {len(b)}"]
        return [d for i, (x, y) in enumerate(zip(a, b)) for d in _walk(x, y, f"{at}{i}.")]
    if isinstance(a, str) and isinstance(b, str) and _mask(a) == _mask(b) and a != b and "eg " in a:
        return []                                                    # a view or line that prints a witness
    return [] if a == b else [f"{at[:-1]}: python {a!r} vs rust {b!r}"]


_REPLAY_DEFS = {}


def replay(eg, draws=None):
    """(ok, reason): does this witness eg line, run in the Python interpreter on the module being compared, give
    exactly the outcome it states? `draws`: the assumed-handler values the witness's model made, fed back in call
    order and then Random(0), as kern/prove.py's render_cex does (the Rust dump records them)."""
    import dataclasses
    from kern.check import run_examples
    from kern.syntax import parse_example
    defs = _REPLAY_DEFS.get("defs")
    if defs is None:
        return False, "no module loaded"
    try:
        ex = parse_example(eg[len("eg "):] if eg.startswith("eg ") else eg, 0)
    except ParseError as e:
        return False, f"unparsable: {e}"
    d = defs.get(ex.call[0])
    if d is None:
        return False, f"calls unknown {ex.call[0]}"
    if draws is not None:
        import kern.check as kc
        import random as _r
        from kern.run import PROVIDERS, Interp
        queue = list(draws)

        def provider(name, args):
            return queue.pop(0) if queue else PROVIDERS[name](_r.Random(0), *args)
        real = kc.Interp
        kc.Interp = lambda defs_: Interp(defs_, provider=provider)
        try:
            res = run_examples(defs, dataclasses.replace(d, examples=[ex]))
        finally:
            kc.Interp = real
    else:
        res = run_examples(defs, dataclasses.replace(d, examples=[ex]))
    return bool(res) and all(r["pass"] for r in res), res[0]["got"] if res else "not run"


def python_dump(path, mutation, hashseed="0"):
    """dump() in a fresh process: within one process, Z3's witness choice depends on earlier solver calls (a known
    limitation, CLAUDE.md), so a module's witnesses would depend on the modules dumped before it."""
    cmd = [sys.executable, os.path.abspath(__file__), "dump", path] + (["--mutation"] if mutation else [])
    p = subprocess.run(cmd, capture_output=True, text=True, env=dict(os.environ, PYTHONHASHSEED=hashseed))
    return json.loads(p.stdout) if p.returncode == 0 else {"error": f"python exited {p.returncode}: {p.stderr[-500:]}"}


def rust_dump(binary, path, mutation):
    if binary == "self":                      # the harness checking itself: Python against Python, another hash seed
        return python_dump(path, mutation, hashseed="12345")
    cmd = [binary, "oracle", "analyse", path] + (["--mutation"] if mutation else [])
    p = subprocess.run(cmd, capture_output=True, text=True)
    if p.returncode != 0:
        return {"error": f"rust exited {p.returncode}: {p.stderr.strip()[-500:]}"}
    return json.loads(p.stdout)


def main(argv):
    ap = argparse.ArgumentParser(prog="oracle_analyse.py")
    ap.add_argument("cmd", choices=["dump", "corpus", "diff"])
    ap.add_argument("path", nargs="?")
    ap.add_argument("--mutation", action="store_true")
    ap.add_argument("--fuzz", type=int, default=60)
    ap.add_argument("--rust")
    ap.add_argument("--only")
    a = ap.parse_args(argv)
    if a.cmd == "dump":
        print(json.dumps(dump(a.path, a.mutation), indent=1, ensure_ascii=False))
        return 0
    paths = corpus(a.fuzz)
    if a.only:
        paths = [p for p in paths if a.only in p]
    if a.cmd == "corpus":
        print("\n".join(os.path.relpath(p, ROOT) for p in paths))
        return 0
    if not a.rust:
        ap.error("diff needs --rust BIN")
    bad = 0
    for p in paths:
        rel = os.path.relpath(p, ROOT)
        py = python_dump(p, a.mutation)
        rs = rust_dump(a.rust, p, a.mutation)
        try:
            _REPLAY_DEFS["defs"] = package.load(p)
            _REPLAY_DEFS["handlers"] = bool(_REPLAY_DEFS["defs"].handlers)
        except ParseError:
            _REPLAY_DEFS["defs"], _REPLAY_DEFS["handlers"] = None, False
        diffs = compare(py, rs, where="")
        bad += bool(diffs)
        print(f"{'ok  ' if not diffs else 'DIFF'} {rel}" + "".join(f"\n       {d}" for d in diffs[:20]))
    print(f"{len(paths) - bad}/{len(paths)} modules agree")
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))

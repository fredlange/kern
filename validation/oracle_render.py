"""Render oracle for the Rust port: Python's reports for the analyse corpus (validation/oracle_analyse.py), with what
views.py, render.py and baseline.py make of them, so toolchain/src/{views,render,baseline}.rs can be checked on the
*same* reports before the Rust prover exists.

    .venv/bin/python validation/oracle_render.py OUT.json [--mutation]

Per module: every report as JSON (oracle_analyse.canon, plus the definition's module and file), the why status of
each definition (from the module's own .why file), the three views, the terminal output, the proof page, the
baseline file `baseline.write` makes, and the same terminal output and page after `baseline.compare` against a
deliberately stronger baseline (so the regression paths render too).
"""
import json
import os
import re
import sys
import tempfile

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, ROOT)

from kern import baseline, package, render, views, why  # noqa: E402
from kern.report import analyse  # noqa: E402
from kern.syntax import ParseError  # noqa: E402
from validation.oracle_analyse import canon, corpus  # noqa: E402


def stronger(text):
    """A baseline every report falls short of: full strength, an extra guarantee and an extra refusal."""
    out = []
    for line in text.splitlines():
        m = re.match(r"^(\w+) (\d+)/(\d+)$", line)
        if m:
            out += [f"{m.group(1)} {m.group(3)}/{m.group(3)}", "  ensure 1 = 1", "  need 0 = 0 else Gone"]
        else:
            out.append(line)
    return "\n".join(out) + "\n"


EXTRA_DIR = os.path.join(ROOT, "build", "oracle_render_extra")
V3 = open(os.path.join(ROOT, "examples/ledger-tour/ledger_v3.k")).read()
V7 = open(os.path.join(ROOT, "examples/ledger-tour/ledger_v7.k")).read()


def extras():
    """Modules for the paths the corpus misses: stale and orphaned why entries, failing examples, a refuted emit,
    a tested-only guarantee with a reason. (path, why file)."""
    import shutil
    shutil.rmtree(EXTRA_DIR, ignore_errors=True)
    os.makedirs(EXTRA_DIR)
    tour = os.path.join(ROOT, "examples/ledger-tour")
    out = [(os.path.join(tour, "ledger_v1.k"), os.path.join(tour, "ledger.why")),
           (os.path.join(tour, "ledger_v3.k"), os.path.join(tour, "ledger_v4.why")),
           (os.path.join(ROOT, "examples/escrow/escrow_s1.k"), os.path.join(tour, "ledger.why"))]
    srcs = {
        "bad_eg": V3.replace("-> ok | A:400 B:100", "-> ok | A:401 B:100", 1),
        "bad_emit": V7.replace("audit.emit (TransferDone from to amount clock.now)", "audit.emit (TransferDone to from amount clock.now)", 1),
        "bad_eg_refusal": V3.replace("-> ok", "-> Insufficient 3", 1),
    }
    for name, src in srcs.items():
        path = os.path.join(EXTRA_DIR, name + ".k")
        open(path, "w").write(src)
        out.append((path, None))
    return out


def dump(path, mutation, whyfile=None):
    try:
        defs = package.load(path)
    except ParseError as e:
        return {"error": str(e)}
    reports = analyse(defs, mutation=mutation)
    store = why.load(whyfile or package.store_path(path if path.endswith(".k") else os.path.join(path, "kern.pkg"), ".why"))
    whys = {n: why.status(store, d, defs) for n, d in defs.items()}
    orphans = why.orphans(store, defs)
    title = os.path.basename(path)
    out = {"title": title, "whys": whys, "orphans": orphans, "reports": {}}
    for n, r in reports.items():
        out["reports"][n] = {**canon(r), "module": getattr(r["def"], "module", None),
                             "file": getattr(r["def"], "file", None),
                             "views": {"iface": views.iface(r, whys[n]), "spec": views.spec(r, whys[n]),
                                       "editor": views.editor(reports, n, whys)}}
    out["terminal"] = render.terminal(reports, whys, orphans)
    out["page"] = render.page(reports, whys, title, orphans)
    with tempfile.NamedTemporaryFile("r", suffix=".strength", delete=False) as f:
        baseline.write(f.name, reports)
        out["baseline"] = open(f.name).read()
    os.unlink(f.name)
    base = baseline.parse(stronger(out["baseline"]))
    baseline.compare(reports, base)
    out["regressed"] = {"base": stronger(out["baseline"]), "regression": {n: r["regression"] for n, r in reports.items()},
                        "terminal": render.terminal(reports, whys, orphans),
                        "page": render.page(reports, whys, title, orphans)}
    return out


def main(argv):
    mutation = "--mutation" in argv
    out = []
    for path, whyfile in [(p, None) for p in corpus()] + extras():
        out.append({"path": os.path.relpath(path, ROOT), "whyfile": whyfile and os.path.relpath(whyfile, ROOT),
                    **dump(path, mutation, whyfile)})
        print(".", end="", flush=True, file=sys.stderr)
    with open(argv[0], "w") as f:
        json.dump(out, f, ensure_ascii=False)
    print(f"\n{len(out)} modules -> {argv[0]}", file=sys.stderr)


if __name__ == "__main__":
    main(sys.argv[1:])

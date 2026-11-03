"""kern — prototype CLI.

  python -m kern check  FILE.k [--why FILE.why] [--html OUT.html]
  python -m kern view   FILE.k NAME --as iface|spec|editor [--why FILE.why]
  python -m kern why    FILE.k NAME [--why FILE.why]
  python -m kern anchor FILE.k --why FILE.why
  python -m kern run    FILE.k "transfer A B 100" --world "A:500@u1 B:0@u1"
"""
import argparse
import os
import sys

from . import render, views, why as whymod
from .report import analyse
from .run import Interp, World
from .syntax import ParseError, parse_module, parse_world


def main(argv=None):
    ap = argparse.ArgumentParser(prog="kern")
    ap.add_argument("cmd", choices=["check", "view", "why", "anchor", "run"])
    ap.add_argument("file")
    ap.add_argument("name", nargs="?")
    ap.add_argument("--why", dest="whyfile")
    ap.add_argument("--html")
    ap.add_argument("--as", dest="view", default="iface", choices=["iface", "spec", "editor"])
    ap.add_argument("--world")
    a = ap.parse_args(argv)
    try:
        defs = parse_module(open(a.file).read())
    except ParseError as e:
        print(f"{a.file}: {e}", file=sys.stderr)
        return 2
    whyfile = a.whyfile or os.path.splitext(a.file)[0] + ".why"
    store = whymod.load(whyfile)
    whys = {n: whymod.status(store, d) for n, d in defs.items()}

    if a.cmd == "anchor":
        whymod.anchor(whyfile, defs)
        print(f"anchored {whyfile} to current builds")
        return 0
    if a.cmd == "run":
        w = World.from_example(parse_world(a.world, None))
        name, *args = a.name.split()
        args = [int(x) if x.lstrip("-").isdigit() else x for x in args]
        outcome, after = Interp(defs).run(name, args, w)
        print(outcome, after.bal, "audit:", after.audit)
        return 0
    if a.cmd == "why":
        w = whys[a.name]
        if w["stale"]:
            print(f"(stale: written for {w['anchored_to']}, code is now {defs[a.name].hash})")
        print("\n".join(w["entries"]) or "(no why entries)")
        return 0

    reports = analyse(defs, mutation=(a.cmd == "check"))
    if a.cmd == "view":
        text = {"iface": lambda: views.iface(reports[a.name], whys[a.name]),
                "spec": lambda: views.spec(reports[a.name], whys[a.name]),
                "editor": lambda: views.editor(reports, a.name, whys)}[a.view]()
        print(text)
        n = views.tokens(text)
        if n is not None:
            print(f"\n[{n} tokens]", file=sys.stderr)
        return 0
    print(render.terminal(reports, whys))
    if a.html:
        with open(a.html, "w") as f:
            f.write(render.page(reports, whys, os.path.basename(a.file)))
        print(f"proof page: {a.html}")
    return 0 if all(r["clean"] for r in reports.values()) else 1


if __name__ == "__main__":
    sys.exit(main())

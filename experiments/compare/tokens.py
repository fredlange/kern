"""Token comparison: Kern escrow_s1 vs behaviour-identical TS and Go. Run from the repo root: .venv/bin/python experiments/compare/tokens.py"""
import sys as _sys, os as _os
_sys.path.insert(0, _os.path.dirname(_os.path.dirname(_os.path.abspath(__file__))))
import os
import re
import statistics

from kern import views
from kern.infer import callees
from kern.report import analyse
from kern.scorecard import NO_WHY, blocks, chain
from kern.syntax import parse_module
from kern.wf import check_module

HERE = os.path.dirname(os.path.abspath(__file__))
T = views.tokens
src = open("examples/escrow/escrow_s1.k").read()
defs = check_module(parse_module(src))
reps = analyse(defs, mutation=False)
ts = open(f"{HERE}/ts/escrow.ts").read()
ts_test = open(f"{HERE}/ts/escrow.test.ts").read()
go = open(f"{HERE}/go/escrow/escrow.go").read()
go_test = open(f"{HERE}/go/escrow/escrow_test.go").read()


def strip_comments(s):
    return re.sub(r"(?m)^\s*(//.*|/\*\*.*\*/)\n", "", s)


# --- writing: what the author produces
spec = sum(T("\n".join(d.src_lines["header"] + [l for l in d.src_lines["spec"] if not l.startswith("eg ")])) for d in defs.values())
eg = sum(T("\n".join(l for l in d.src_lines["spec"] if l.startswith("eg "))) for d in defs.values())
body = sum(T("\n".join(d.src_lines["body"])) for d in defs.values())
print("WRITING (tokens)")
print(f"  Kern  source {T(src)}  = spec {spec} + examples {eg} + body {body}  (+ headers/whitespace)")
print(f"  TS    code {T(ts)} (no comments {T(strip_comments(ts))})  + tests {T(ts_test)}  = {T(ts) + T(ts_test)}")
print(f"  Go    code {T(go)} (no comments {T(strip_comments(go))})  + tests {T(go_test)}  = {T(go) + T(go_test)}")


# --- reading: per definition
def ts_fn(name):
    return re.search(rf"(?s)(/\*\*[^\n]*\*/\n)?export function {name}\(.*?\n}}\n", ts).group(0)


def go_fn(name):
    return re.search(rf"(?s)(// [^\n]*\n)?func \(s \*Service\) {name[0].upper() + name[1:]}\(.*?\n}}\n", go).group(0)


ts_types = ts[ts.index("export type EscrowEvent"):ts.index("function needFunds")]
ts_help = re.search(r"(?s)function needFunds\(.*?\n}\n", ts).group(0)
go_types = go[go.index("var ("):go.index("func (s *Service) needFunds")]
go_help = re.search(r"(?s)func \(s \*Service\) needFunds\(.*?\n}\n", go).group(0)
kern_blocks = blocks(src)


def closure(fn, types, helper, n):
    parts = [fn(x) for x in chain(defs, n)]
    if any("needFunds" in p for p in parts):
        parts.append(helper)
    return T(types + "\n".join(parts))


rows = []
for n in defs:
    rows.append({
        "def": n, "callees": len(chain(defs, n)) - 1,
        "kern_spec": T(views.spec(reps[n], NO_WHY)), "kern_editor": T(views.editor(reps, n, {m: NO_WHY for m in defs})),
        "kern_src_closure": T("\n\n".join(kern_blocks[x] for x in chain(defs, n))),
        "ts_closure": closure(ts_fn, ts_types, ts_help, n), "go_closure": closure(go_fn, go_types, go_help, n),
        "ts_sig": T(ts_fn(n).split("{")[0]), "go_sig": T(go_fn(n).split("{")[0]),
    })
print("\nREADING per definition (tokens)")
print(f"  {'def':<9}{'callees':>8} | {'Kern spec':>9}{'Kern edit':>10}{'Kern src*':>10} | {'TS sig':>7}{'TS clos':>8} | {'Go sig':>7}{'Go clos':>8}")
for r in rows:
    print(f"  {r['def']:<9}{r['callees']:>8} | {r['kern_spec']:>9}{r['kern_editor']:>10}{r['kern_src_closure']:>10} | "
          f"{r['ts_sig']:>7}{r['ts_closure']:>8} | {r['go_sig']:>7}{r['go_closure']:>8}")
print("  * Kern src = the definition plus every transitive callee, full source (no views)")
for k in ("kern_spec", "kern_editor", "ts_closure", "go_closure"):
    print(f"  median {k:<12} {statistics.median(r[k] for r in rows)}")
deep = [r for r in rows if r["callees"] >= 2]
for k in ("kern_editor", "ts_closure", "go_closure"):
    print(f"  median with >=2 callees ({len(deep)} defs) {k:<12} {statistics.median(r[k] for r in deep)}")
print("\nLANGUAGE TAX: CLAUDE.md (the only Kern reference an agent gets)", T(open("CLAUDE.md").read()))

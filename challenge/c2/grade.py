"""C2 grader (frozen before any run). Scripts only; the agent being graded never sees hidden/.

    .venv/bin/python challenge/c2/grade.py ARM TASK WORKSPACE [--transcript run.json] [--mcp-log log.jsonl]
    .venv/bin/python challenge/c2/grade.py --validate        (grades reference, untouched and a bad variant)

ARM is kview, kfull or ts. For each finished workspace it reports:
- success: every hidden acceptance case matches (outcome with refusal arguments, balances, events);
- guarantees broken: the ORIGINAL escrow_s1 specs (keep / ensure / emit / ok otherwise), evaluated on
  1,000 random calls to the original definitions, outside the inputs the task legitimately changes.
  The same check applies to Kern and TypeScript results (the Kern specs are TypeScript's grader);
- visible: for a Kern result, whether the definition whose guarantee broke had its spec changed
  (the ratchet would show it); TypeScript has no spec to change;
- kern check: whether the final module is clean (Kern arms);
- from the transcript and the MCP log: tokens, and check iterations (and whether the first was clean).
"""
import json
import os
import sys
import tempfile

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from common import ROOT, load_kern, random_cases, run_kern, run_ts, workspace  # noqa: E402

from kern.report import analyse  # noqa: E402
from kern.run import Interp, World  # noqa: E402
from kern.syntax import parse_module, stmt_code  # noqa: E402

HERE = os.path.dirname(os.path.abspath(__file__))
ORIG = load_kern(f"{ROOT}/examples/escrow_s1.k")


def exempt(task, c):
    """Inputs whose behaviour the task itself changes: the original guarantee needn't hold there."""
    d, a = c["def"], c["args"]
    if task == "T2":
        if d in ("capture", "purchase"):
            return a[-2] >= 1000 and a[-1] > 50
        if d == "refund":
            amount, part, fee = a[4], a[5], a[6]
            return part < amount and amount - part >= 1000 and fee > 50
    if task == "T3":                  # only move itself: tip A A 0 is the trap T3 measures (its
        if d == "move":               # `ok otherwise` breaks unless the change to it is made visible)
            return a[0] == a[1] and a[2] == 0
    return False


def guarantee_breaks(task, cases, results, new_defs=None):
    """Original guarantees that the result violates, outside the exempt inputs."""
    interp, breaks = Interp(ORIG), []
    for c, r in zip(cases, results):
        if exempt(task, c) or r["outcome"][0] in ("missing", "error"):
            continue
        d0 = ORIG[c["def"]]
        pre = World(dict(c["world"]), {a: "u" for a in c["world"]}, c["now"], [], {})
        env = {p: v for (p, _), v in zip(d0.params, c["args"])}
        if r["outcome"] == ["ok"]:
            post = World(dict(r["bal"]), dict(pre.own), c["now"], [tuple(e) for e in r["events"]], {})
            for kind, clause in interp.holds(d0, c["args"], pre, post):
                breaks.append((c["def"], f"{kind} {clause}", c))
        elif d0.complete and all(interp.ev(n[1], env, pre.copy()) for n in d0.needs):
            breaks.append((c["def"], "ok otherwise", c))
    out = {}
    for d, g, c in breaks:
        key = f"{d}: {g}"
        out.setdefault(key, {"count": 0, "example": c})["count"] += 1
    if new_defs is not None:                         # visible: the definition's spec changed
        for key in out:
            d = key.split(":")[0]
            nd = new_defs.get(d)
            out[key]["visible"] = nd is None or _spec(nd) != _spec(ORIG[d])
    return out


def _spec(d):
    return ([stmt_code(n) for n in d.needs], d.complete, [repr(x) for x in d.keeps + d.ensures], repr(d.emits))


def grade(arm, task, ws, transcript=None, mcp_log=None):
    hidden = json.load(open(f"{HERE}/hidden/{task}.json"))
    reg = random_cases(ORIG, 1000, seed=202)
    out = {"arm": arm, "task": task}
    if arm == "ts":
        got, err = run_ts(f"{ws}/ts", hidden["cases"])
        reg_got, err2 = run_ts(f"{ws}/ts", reg)
        if err or err2:
            return dict(out, success=False, error=(err or err2)[-400:])
        new_defs = None
    else:
        try:
            defs = load_kern(f"{ws}/escrow_s1.k")
        except Exception as e:                       # the module doesn't parse or check
            return dict(out, success=False, error=f"module: {e}"[:400])
        got, reg_got, new_defs = run_kern(defs, hidden["cases"]), run_kern(defs, reg), defs
        reps = analyse(defs, mutation=False)
        out["kern_check_clean"] = all(r["clean"] for r in reps.values())
    match = [g == e for g, e in zip(got, hidden["expected"])]
    out["acceptance"] = f"{sum(match)}/{len(match)}"
    out["success"] = all(match)
    breaks = guarantee_breaks(task, reg, reg_got, new_defs)
    out["guarantees_broken"] = {k: {"count": v["count"], "visible": v.get("visible")} for k, v in breaks.items()}
    out["guarantees_broken_silently"] = sorted(k for k, v in breaks.items() if v.get("visible") is not True)
    if transcript:
        t = json.load(open(transcript))
        u = t.get("usage", {})
        out["tokens"] = {"input": u.get("input_tokens", 0) + u.get("cache_read_input_tokens", 0)
                         + u.get("cache_creation_input_tokens", 0), "output": u.get("output_tokens", 0),
                         "cost_usd": t.get("total_cost_usd"), "turns": t.get("num_turns")}
    if mcp_log and os.path.exists(mcp_log):
        rows = [json.loads(l) for l in open(mcp_log)]
        checks = [r for r in rows if r["tool"] == "kern_check"]
        out["mcp"] = {"calls": len(rows), "response_tokens": sum(r["response_tokens"] or 0 for r in rows),
                      "check_iterations": len(checks)}
    return out


def validate():
    """Before any run: the grader must pass the references, fail untouched code, and catch a silent break."""
    ref = json.load(open(f"{HERE}/hidden/reference/solutions.json"))
    rows = []
    for task in ("T1", "T2", "T3"):
        for label, kedits, tedits in (("reference", ref[task]["kern"], ref[task]["ts"]), ("untouched", None, None)):
            with tempfile.TemporaryDirectory() as tmp:
                workspace(tmp, kedits, tedits)
                for arm in ("kfull", "ts"):
                    g = grade(arm, task, tmp)
                    rows.append((task, label, arm, g["success"], g["acceptance"], g["guarantees_broken"],
                                 g.get("kern_check_clean")))
    # a silent break: T3 done in move only (Kern: tip's restated need left as it was)
    with tempfile.TemporaryDirectory() as tmp:
        workspace(tmp, [ref["T3"]["kern"][0]], [ref["T3"]["ts"][0]])
        for arm in ("kfull", "ts"):
            g = grade(arm, "T3", tmp)
            rows.append(("T3", "move-only", arm, g["success"], g["acceptance"], g["guarantees_broken"],
                         g.get("kern_check_clean")))
    for r in rows:
        print(*r)
    return rows


if __name__ == "__main__":
    if sys.argv[1:] == ["--validate"]:
        validate()
    else:
        arm, task, ws = sys.argv[1:4]
        opts = dict(zip(sys.argv[4::2], sys.argv[5::2]))
        print(json.dumps(grade(arm, task, ws, opts.get("--transcript"), opts.get("--mcp-log")), indent=1))

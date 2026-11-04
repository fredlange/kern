"""C1 grader (frozen before any bug is planted). Deterministic; reads only patches/*.json.

    .venv/bin/python challenge/c1/grade.py [patches/B01.json ...]     (from the repo root)

For each patch: apply it to a temp copy of escrow_s1.k, compare/ts and compare/go; check that
it is live and that the three languages agree (5,000 cases, seed 1, the generator of
compare/diff.py); then run `kern check` (mutation testing and the ratchet against the
original's baseline), `node --test` and `go test`. Writes results.json and prints a table.
"""
import glob
import json
import os
import random
import shutil
import subprocess
import sys
import tempfile

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
sys.path.insert(0, ROOT)
from kern import baseline as basemod  # noqa: E402
from kern.report import analyse  # noqa: E402
from kern.run import Interp, World  # noqa: E402
from kern.syntax import ParseError, parse_module  # noqa: E402
from kern.wf import check_module  # noqa: E402

HERE = os.path.dirname(os.path.abspath(__file__))
ACC = ["A", "B", "E", "H"]
INTS = [-1, 0, 1, 5, 10, 50, 99, 100, 101, 150, 200, 300, 600]
N, SEED = 5000, 1


def cases(defs):
    rng = random.Random(SEED)
    out = []
    for i in range(N):
        d = rng.choice(list(defs.values()))
        world = {a: rng.choice([0, 0, 5, 50, 100, 200, 300, 600]) for a in ACC}
        args = [rng.choice(ACC) if t == "Account" else rng.choice(INTS) for _, t in d.params]
        out.append({"def": d.name, "args": args, "world": world, "now": i})
    return out


def run_kern(defs, cs):
    interp, out = Interp(defs), []
    for c in cs:
        w = World(dict(c["world"]), {a: "u" for a in ACC}, c["now"], [], {})
        outcome, w = interp.run(c["def"], c["args"], w)
        out.append({"outcome": ["ok"] if outcome == ("ok",) else [outcome[0], list(outcome[1])],
                    "bal": w.bal, "events": [list(e) for e in w.audit]})
    return out


def run_ext(tmp, cs):
    inp = "\n".join(json.dumps(c) for c in cs) + "\n"
    res = {}
    b = subprocess.run(["go", "build", "-o", "diffbin", "./cmd/diff"], cwd=f"{tmp}/go", capture_output=True, text=True)
    for name, cmd, cwd in (("ts", ["node", "diff.ts"], f"{tmp}/ts"), ("go", ["./diffbin"], f"{tmp}/go")):
        if name == "go" and b.returncode:
            res[name] = {"error": "build: " + b.stderr[-500:]}
            continue
        p = subprocess.run(cmd, input=inp, capture_output=True, text=True, cwd=cwd)
        res[name] = [json.loads(l) for l in p.stdout.splitlines()] if p.returncode == 0 else {"error": p.stderr[-500:]}
    return res


def apply(path, edits):
    s = open(path).read()
    for old, new in edits:
        if s.count(old) != 1:
            raise ValueError(f"{os.path.basename(path)}: `{old[:60]}` occurs {s.count(old)} times, want 1")
        s = s.replace(old, new)
    open(path, "w").write(s)


def mechanisms(reps):
    m = set()
    for r in reps.values():
        if any(g["status"] != "proven" for g in r["guarantees"]):
            m.add("proof")
        if not all(e["pass"] for e in r["examples"]):
            m.add("eg")
        if r["props"]["violations"]:
            m.add("random")
        if r.get("regression"):
            m.add("ratchet")
    return sorted(m)


def grade(patch, orig_defs, orig_kern, cs, base):
    p = json.load(open(patch))
    tmp = tempfile.mkdtemp(prefix=f"c1_{p['id']}_")
    try:
        shutil.copytree(f"{ROOT}/compare/ts", f"{tmp}/ts")
        shutil.copytree(f"{ROOT}/compare/go", f"{tmp}/go", ignore=shutil.ignore_patterns("diffbin"))
        shutil.copy(f"{ROOT}/examples/escrow_s1.k", f"{tmp}/escrow_s1.k")
        out = {"id": p["id"], "source": p["source"], "category": p["category"], "kern_where": p["kern_where"],
               "description": p["description"]}
        apply(f"{tmp}/ts/escrow.ts", p["ts"])
        apply(f"{tmp}/go/escrow/escrow.go", p["go"])
        results = run_ext(tmp, cs)
        mdefs = None
        if p["kern"] is not None:
            apply(f"{tmp}/escrow_s1.k", p["kern"])
            try:
                mdefs = check_module(parse_module(open(f"{tmp}/escrow_s1.k").read()))
                results["kern"] = run_kern(mdefs, cs)
            except ParseError as e:                     # includes WellFormedError
                out["kern_illformed"] = str(e)[:300]
        live, errors = {}, {}
        for lang, got in results.items():
            if isinstance(got, dict):
                errors[lang] = got["error"]
                continue
            live[lang] = sum(a != b for a, b in zip(got, orig_kern))
        out["changed_cases"] = live
        out["errors"] = errors
        langs = [l for l in results if not isinstance(results[l], dict)]
        out["agree"] = all(results[langs[0]] == results[l] for l in langs[1:]) if langs else False
        out["live"] = bool(live) and all(v > 0 for v in live.values())
        # existing tests
        out["ts_caught"] = subprocess.run(["node", "--test", "escrow.test.ts"], cwd=f"{tmp}/ts",
                                          capture_output=True).returncode != 0
        out["go_caught"] = subprocess.run(["go", "test", "./escrow"], cwd=f"{tmp}/go",
                                          capture_output=True).returncode != 0
        if mdefs is not None:
            reps = analyse(mdefs, mutation=True, baseline=base)
            out["kern_mechanisms"] = mechanisms(reps)
            out["kern_caught"] = bool(out["kern_mechanisms"])
            out["kern_not_clean"] = sorted(n for n, r in reps.items() if not r["clean"] or r.get("regression"))
        elif p["kern"] is not None:
            out["kern_mechanisms"], out["kern_caught"] = ["ill-formed"], True
        return out
    finally:
        shutil.rmtree(tmp, ignore_errors=True)


def summary(rows):
    live = [r for r in rows if r["live"] and r["agree"]]
    expr = [r for r in live if r["kern_where"] != "none"]
    body = [r for r in expr if r["kern_where"] == "body"]
    pe = lambda r: bool(set(r.get("kern_mechanisms", [])) & {"proof", "eg"})
    rate = lambda rs: (sum(map(pe, rs)), len(rs))
    s = {
        "patches": len(rows), "live_and_agreeing": len(live),
        "not_live": [r["id"] for r in rows if not r["live"]],
        "disagree": [r["id"] for r in rows if r["live"] and not r["agree"]],
        "not_expressible_in_kern": [r["id"] for r in live if r["kern_where"] == "none"],
        "P1_body_proof_or_eg": rate(body),
        "P1_catalogue": rate([r for r in body if r["source"] == "catalogue"]),
        "P1_blind": rate([r for r in body if r["source"] == "blind"]),
        "kern_caught_any_mechanism": (sum(r["kern_caught"] for r in expr), len(expr)),
        "ts_caught": (sum(r["ts_caught"] for r in expr), len(expr)),
        "go_caught": (sum(r["go_caught"] for r in expr), len(expr)),
        "spec_located": [(r["id"], r["kern_caught"], r.get("kern_mechanisms")) for r in expr if r["kern_where"] == "spec"],
    }
    k, t, g = s["kern_caught_any_mechanism"][0], s["ts_caught"][0], s["go_caught"][0]
    b, bn = s["P1_body_proof_or_eg"]
    s["P1_pass"] = bn > 0 and b / bn >= 0.9
    s["P2_pass"] = k > t and k > g
    cat, bl = s["P1_catalogue"], s["P1_blind"]
    if cat[1] and bl[1]:
        s["P3_gap_points"] = round(100 * (cat[0] / cat[1] - bl[0] / bl[1]), 1)
        s["P3_author_bias_finding"] = s["P3_gap_points"] > 15
    return s


def main():
    paths = sys.argv[1:] or sorted(glob.glob(f"{HERE}/patches/*.json"))
    orig = check_module(parse_module(open(f"{ROOT}/examples/escrow_s1.k").read()))
    cs = cases(orig)
    orig_kern = run_kern(orig, cs)
    bfile = tempfile.mktemp(suffix=".strength")
    basemod.write(bfile, analyse(orig, mutation=True))
    base = basemod.load(bfile)
    rows = []
    for path in paths:
        r = grade(path, orig, orig_kern, cs, base)
        rows.append(r)
        print(f"{r['id']:4} {r['source']:9} {r['kern_where']:5} live={r['live']!s:5} agree={r['agree']!s:5} "
              f"kern={','.join(r.get('kern_mechanisms', [])) or '-':22} ts={r['ts_caught']!s:5} go={r['go_caught']!s:5} "
              f"{r['changed_cases']} {r['errors'] or ''}", flush=True)
    s = summary(rows)
    json.dump({"rows": rows, "summary": s}, open(f"{HERE}/results.json", "w"), indent=1)
    print(json.dumps(s, indent=1))


if __name__ == "__main__":
    main()

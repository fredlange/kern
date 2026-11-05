"""C2b's prediction, written before any C2b run (2026-11-05). Reads C2's recorded runs and git; writes nothing.

    .venv/bin/python -I experiments/challenge/c2b/predict.py \
        --tokenizer .venv/lib/python3.9/site-packages/anthropic/tokenizer.json

`kern` is target/release/kern or the one on PATH; KERN_BIN=/path/to/kern overrides it (the numbers in PROTOCOL.md
were measured with a binary built from a clean `git archive` of df332d5).

It asks what C2's own runs would have cost had the module been escrow_s3 (55 definitions) instead of escrow_s1
(10), with every agent's behaviour held fixed: the same calls in the same order, plus the extra reading that s3's
larger neighbourhoods force (PROTOCOL.md, "Prediction", lists every assumption). Each recorded tool result is
replaced by what the same call returns on s3, and the difference is priced with C2's own calibration (view_cost.py,
section 6: 1.65 real tokens per proxy token of tool-result text, 126 per tool call).

- K-view: `kern_list`, `kern_callers` and `kern_check` results are re-served by `kern mcp` on s1 and on s3 (the
  difference is added); views are replayed on s3 through the server's session dedup, in two designs: the agents
  keep asking for `editor` (as all 35 C2 views did), or they follow kern_view's description (`spec`, `editor` only
  for definitions they then change). A sensitivity line adds a `spec` view of every direct caller of what the
  task changes.
- K-full: whole-file reads of escrow_s1.k grow by s3/s1, measured; `kern check` output by the measured difference
  between s1 and s3 (Rust toolchain, CLI).
- TS: there is no TypeScript port of s3 yet. Whole-file reads (Read without a window, `cat`) of escrow.ts and
  escrow.test.ts grow by an assumed ratio (Kern s3/s1 for the code, eg lines s3/s1 for the tests); partial reads
  stay the same size. That ratio is the prediction's weakest input, so the script also prints each margin: how
  much less (or more) TS would have to read for the result to flip at 0.7x.
- Cost: per-token prices fitted on C2's 27 runs (exact on every run); cache reads scaled with the predicted total.
"""
import json
import os
import re
import statistics as st
import subprocess
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, "..", "c2"))
import view_cost as vc  # noqa: E402

ROOT = vc.ROOT
if os.environ.get("KERN_BIN"):          # measure with a given binary (e.g. one built from a clean checkout)
    vc.kern_exe = lambda: os.environ["KERN_BIN"]
S3 = "examples/escrow/escrow_s3.k"
REAL_PER_PROXY_PROSE = 1.31          # REFERENCE.md's calibration (PROTOCOL.md of C2, first dated note)

# What the s3 versions of C2's tasks add to each run's reading (PROTOCOL.md, "Tasks" and "Prediction").
TARGETS = {"T1": set(), "T2": {"fee_for"}, "T3": {"move", "tip", "charge_late"}}
EXTRA_VIEWS = {"T1": [], "T2": ["fee_for"], "T3": ["charge_late", "checkout_vat", "payout_holdback"]}
EXTRA_EDITS = {"T1": 0, "T2": 0, "T3": 1}   # charge_late needs the same guard as tip


def mcp(calls, tok_path, root):
    """One `kern mcp` session; returns each call's text."""
    lines = ['{"jsonrpc":"2.0","id":0,"method":"initialize","params":{}}']
    for i, (tool, args) in enumerate(calls, 1):
        lines.append(json.dumps({"jsonrpc": "2.0", "id": i, "method": "tools/call",
                                 "params": {"name": tool, "arguments": args}}))
    env = dict(os.environ, KERN_TOKENIZER=tok_path, KERN_MCP_ROOT=root)
    out = subprocess.run([vc.kern_exe(), "mcp"], input="\n".join(lines) + "\n", capture_output=True, text=True,
                         env=env, cwd=root).stdout
    got = {}
    for line in out.splitlines():
        m = json.loads(line)
        if m.get("id"):
            got[m["id"]] = m["result"]["content"][0]["text"]
    return [got[i] for i in range(1, len(calls) + 1)]


def share(text, lines):
    """The fraction of a tool result's lines that are lines of a given file (Read's line-number prefix removed)."""
    rows = [re.sub(r"^\s*\d+(→|\t)", "", r).strip() for r in text.splitlines()]
    rows = [r for r in rows if r]
    return sum(r in lines for r in rows) / len(rows) if rows else 0.0


def whole_read(tool, inp):
    """A read that would grow with the file: Read without offset/limit, or a shell command that cats it. Partial
    reads (Read with a window, sed/grep/python snippets) are assumed to stay the same size on s3."""
    if tool == "Read":
        return not (inp.get("offset") or inp.get("limit"))
    return tool == "Bash" and re.search(r"(^|[;&|]\s*|\s)cat\s", inp.get("command", "")) is not None


def file_lines(text):
    return {r.strip() for r in text.splitlines() if len(r.strip()) > 8}


def main():
    n = vc.tokenizer()
    if n is None:
        sys.exit("predict.py needs the tokenizer")
    tok_path = os.path.abspath(sys.argv[sys.argv.index("--tokenizer") + 1] if "--tokenizer" in sys.argv else
                               os.environ.get("KERN_TOKENIZER") or os.path.expanduser("~/.cache/kern/tokenizer.json"))
    # some K-full/TS transcript lines carry `message` as a string; tool_results only reads dict messages
    data = {k: (u, f, [m for m in msgs if isinstance(m.get("message", {}), dict)])
            for k, (u, f, msgs) in vc.runs().items()}
    rate, (_, per_call, _, _) = vc.fit_marginal(data, n)
    rev = subprocess.run(["git", "-C", ROOT, "rev-parse", "--short", "HEAD"], capture_output=True, text=True).stdout.strip()
    print(f"calibration (C2's K-view read-only turns): {rate:.2f} real per proxy token, {per_call:.0f} per tool call")
    print(f"s3 module: {S3} at {rev}; kern binary: {vc.kern_exe()}")

    tmp = tempfile.mkdtemp(prefix="c2b_predict_")
    s1, s3 = os.path.join(tmp, "escrow_s1.k"), os.path.join(tmp, "escrow_s3.k")
    s1_text = vc.git_show("examples/escrow_s1.k")
    s3_text = open(os.path.join(ROOT, S3)).read()
    open(s1, "w").write(s1_text)
    open(s3, "w").write(s3_text)
    r_kern = n(s3_text) / n(s1_text)
    eg = lambda t: sum(1 for line in t.splitlines() if line.startswith("  eg "))
    r_test = eg(s3_text) / eg(s1_text)
    print(f"module size, proxy tokens: s1 {n(s1_text):,}, s3 {n(s3_text):,} ({r_kern:.2f}x); eg lines {eg(s1_text)} -> "
          f"{eg(s3_text)} ({r_test:.2f}x)")

    # ---- fixed per-call differences, same server on both modules
    per = {}
    for name, (tool, args) in {"list": ("kern_list", {}), "check": ("kern_check", {})}.items():
        a, b = mcp([(tool, dict(args, file=s1))], tok_path, tmp)[0], mcp([(tool, dict(args, file=s3))], tok_path, tmp)[0]
        per[name] = (n(a), n(b))
    print(f"kern_list {per['list'][0]:,} -> {per['list'][1]:,}; kern_check (clean) {per['check'][0]:,} -> "
          f"{per['check'][1]:,} proxy tokens (s1 -> s3, `kern mcp` now)")
    cli = {}
    for f in (s1, s3):
        out = subprocess.run([vc.kern_exe(), "check", f], capture_output=True, text=True, cwd=tmp)
        cli[f] = n(out.stdout + out.stderr)
    print(f"`kern check` CLI output {cli[s1]:,} -> {cli[s3]:,}")
    ref_then, ref_now = n(vc.git_show("docs/REFERENCE.md")), n(open(os.path.join(ROOT, "docs", "REFERENCE.md")).read())
    first_delta = (ref_now - ref_then) * REAL_PER_PROXY_PROSE
    print(f"REFERENCE.md {ref_then:,} -> {ref_now:,} proxy: +{first_delta:,.0f} real on the first request of the Kern arms")

    # the TS arm's files at the run (the agents changed their workspace copies)
    ts_lines = file_lines(vc.git_show("compare/ts/escrow.ts"))
    test_lines = file_lines(vc.git_show("compare/ts/escrow.test.ts"))
    k_lines = file_lines(s1_text)

    cache = {}

    def callers_s3(names):
        out = set()
        for x in names:
            if x not in cache:
                cache[x] = json.loads(mcp([("kern_callers", {"file": s3, "name": x})], tok_path, tmp)[0])["direct"]
            out |= set(cache[x])
        return out - set(names)

    pred = {}
    for k in sorted(data):
        arm, task, seed = k
        usage, final, msgs = data[k]
        total = final["input_tokens"] + final["cache_creation_input_tokens"] + final["output_tokens"]
        results = vc.tool_results(msgs)
        if arm == "ts":
            grow = {"whole": 0.0, "all": 0.0}
            for (tool, inp), text in results:
                t = n(text)
                if tool == "Bash" and "node --test" in inp.get("command", ""):
                    g = w = (r_test - 1) * t
                else:
                    g = (r_kern - 1) * t * share(text, ts_lines) + (r_test - 1) * t * share(text, test_lines)
                    w = g if whole_read(tool, inp) else 0.0
                grow["all"] += g
                grow["whole"] += w
            pred[k] = {"ts": total + rate * grow["whole"], "ts, every read grows": total + rate * grow["all"]}
        elif arm == "kfull":
            grow = 0.0
            for (tool, inp), text in results:
                t = n(text)
                if tool == "Bash" and " check" in inp.get("command", "") and "kern" in inp.get("command", ""):
                    grow += cli[s3] - cli[s1]
                elif whole_read(tool, inp):
                    grow += (r_kern - 1) * t * share(text, k_lines)
            pred[k] = {"kfull": total + first_delta + rate * grow}
        else:
            recorded_views, names, grow, calls = 0, [], 0.0, 0
            for (tool, inp), text in results:
                t = n(text)
                if tool.startswith("kern_view"):
                    recorded_views += t
                    names.append(inp["name"])
                elif tool == "kern_list":
                    grow += per["list"][1] - per["list"][0]
                elif tool == "kern_check":
                    grow += per["check"][1] - per["check"][0]
                elif tool == "kern_callers":
                    a, b = (mcp([("kern_callers", {"file": f, "name": inp["name"]})], tok_path, tmp)[0] for f in (s1, s3))
                    grow += n(b) - n(a)
            seq = names + [x for x in EXTRA_VIEWS[task] if x not in names]
            calls += len(seq) - len(names)
            editor = [("kern_view", {"file": s3, "name": x, "as": "editor"}) for x in seq]
            spec_first = [("kern_view", {"file": s3, "name": x, "as": "editor" if x in TARGETS[task] else "spec"})
                          for x in seq]
            v_editor = sum(n(r) for r in mcp(editor, tok_path, tmp))
            v_spec = sum(n(r) for r in mcp(spec_first, tok_path, tmp))
            # wide: the agent also reads (as spec) every direct caller of each definition it changes
            wide = seq + [c for c in sorted(callers_s3(TARGETS[task])) if c not in seq]
            calls_wide = len(wide) - len(seq)
            v_wide = sum(n(r) for r in mcp([("kern_view", {"file": s3, "name": x, "as": "editor" if x in TARGETS[task]
                                                            else "spec"}) for x in wide], tok_path, tmp))
            # an extra edit costs what this run's median replace op cost in output (proxy of its JSON)
            ops = [o for (tool, inp), _ in results if tool in ("kern_edit", "kern_replace")
                   for o in (inp.get("ops") or [dict(inp, op="replace")]) if o.get("op") == "replace"]
            edit_cost = st.median(n(json.dumps(o)) for o in ops) if ops else 0
            extra = EXTRA_EDITS[task] * (rate * edit_cost + per_call) + calls * per_call
            base = total + first_delta + rate * grow + extra
            pred[k] = {"kview, editor": base + rate * (v_editor - recorded_views),
                       "kview, spec-first": base + rate * (v_spec - recorded_views),
                       "kview, spec-first, wide": base + rate * (v_wide - recorded_views) + calls_wide * per_call,
                       "_views": (recorded_views, v_editor, v_spec)}

    print("\nPredicted totals under the frozen metric (input + cache_creation + output), median of C2's three seeds")
    rows = {}
    for task in vc.TASKS:
        med = lambda arm, key: st.median(pred[(arm, task, s)][key] for s in range(3))
        ts = med("ts", "ts")
        ke, ks, kf = med("kview", "kview, editor"), med("kview", "kview, spec-first"), med("kfull", "kfull")
        kw, ts_all = med("kview", "kview, spec-first, wide"), med("ts", "ts, every read grows")
        views = [pred[("kview", task, s)]["_views"] for s in range(3)]
        rows[task] = (ts, ke, ks, kf)
        print(f"  {task}: TS {ts:,.0f} (0.7x = {0.7 * ts:,.0f}) | K-view, editor {ke:,.0f} ({ke / ts:.2f}x) | "
              f"K-view, spec-first {ks:,.0f} ({ks / ts:.2f}x) | K-full {kf:,.0f} ({kf / ts:.2f}x)")
        print(f"      sensitivity: K-view spec-first, also reading every direct caller of what it changes "
              f"({', '.join(sorted(callers_s3(TARGETS[task]))) or 'none'}): {kw:,.0f} ({kw / ts:.2f}x); "
              f"TS if partial reads grew too: {ts_all:,.0f} (K-view spec-first {ks / ts_all:.2f}x)")
        print(f"      view tokens (proxy, median): C2 recorded {st.median(v[0] for v in views):,.0f}, s3 editor "
              f"{st.median(v[1] for v in views):,.0f}, s3 spec-first {st.median(v[2] for v in views):,.0f}")
        for label, kv in (("editor", ke), ("spec-first", ks)):
            margin = (ts - kv / 0.7) / rate
            print(f"      margin ({label}): K-view meets 0.7x while TS's total is at least {kv / 0.7:,.0f}; "
                  + (f"TS could read {margin:,.0f} proxy tokens less than modelled and K-view would still pass"
                     if margin >= 0 else f"TS would have to read {-margin:,.0f} proxy tokens more than modelled"))
    print("\nTS's assumed growth: whole-file reads holding escrow.ts lines x%.2f, escrow.test.ts lines and `node --test` output "
          "x%.2f. No TS port of s3 exists; its real size replaces these ratios before the run." % (r_kern, r_test))

    # ---- cost: per-token prices fitted on C2's 27 runs (cost_usd against the four usage numbers, no intercept);
    # cache reads scaled with each run's predicted total (they grow with context size x turns)
    res = json.load(open(os.path.join(vc.HERE, "results.json")))
    keys = ("input_tokens", "cache_creation_input_tokens", "cache_read_input_tokens", "output_tokens")
    X = [[r["usage"][k] for k in keys] for r in res.values()]
    Y = [r["cost_usd"] for r in res.values()]
    A = [[sum(x[i] * x[j] for x in X) for j in range(4)] + [sum(x[i] * y for x, y in zip(X, Y))] for i in range(4)]
    for i in range(4):
        piv = max(range(i, 4), key=lambda r: abs(A[r][i]))
        A[i], A[piv] = A[piv], A[i]
        for r in range(4):
            if r != i:
                f = A[r][i] / A[i][i]
                A[r] = [a - f * b for a, b in zip(A[r], A[i])]
    price = [A[i][4] / A[i][i] for i in range(4)]
    fit_err = max(abs(sum(p * v for p, v in zip(price, x)) - y) for x, y in zip(X, Y))
    print("\nCost: fitted $ per million tokens (input, cache_creation, cache_read, output): "
          + ", ".join(f"{p * 1e6:.2f}" for p in price) + f" (largest error on a C2 run ${fit_err:.3f})")
    per_cell = {}
    for k, v in pred.items():
        rec = res[f"{k[0]}_{k[1]}_{k[2]}"]
        tot = max(x for key, x in v.items() if not key.startswith("_"))
        scale = tot / rec["tokens"]
        u = rec["usage"]
        c = (price[0] * u["input_tokens"] + price[1] * (tot - u["input_tokens"] - u["output_tokens"])
             + price[2] * u["cache_read_input_tokens"] * scale + price[3] * u["output_tokens"])
        per_cell.setdefault(k[0], []).append(c)
    for arm, cs in sorted(per_cell.items()):
        print(f"  {arm}: ${min(cs):.2f}-{max(cs):.2f} per run (C2: "
              f"${min(r['cost_usd'] for r in res.values() if r['arm'] == arm):.2f}-"
              f"{max(r['cost_usd'] for r in res.values() if r['arm'] == arm):.2f})")

if __name__ == "__main__":
    main()

"""Where K-view's tokens went in C2 (added 2026-11-05, after the run; reads runs/ and git only, writes nothing).

    python3 -I experiments/challenge/c2/view_cost.py [--tokenizer PATH]

Usage numbers (cache_read / cache_creation per request) come from runs/*/workspace/{transcript.jsonl,final.json}
and need nothing extra. Token counts of tool results, schemas, REFERENCE.md and escrow_s1.k use the Claude
tokenizer bundled in anthropic==0.34.2 (a proxy: the API's own counts are ~1.2-1.3x higher here), loaded with
the `tokenizers` package from --tokenizer, KERN_TOKENIZER or ~/.cache/kern/tokenizer.json. Without either, those
sections are skipped. The interpreter in the old checkout's .venv has `tokenizers`:

    .venv/bin/python -I experiments/challenge/c2/view_cost.py \
        --tokenizer .venv/lib/python3.9/site-packages/anthropic/tokenizer.json

The tool schemas are read from `kern mcp` (target/release/kern, or `kern` on PATH) when it is available; the
module and REFERENCE.md are read at the run's revision (C2_REV) with git.

Section 6 asks how much cheaper the K-view runs' reading would have been under other view designs: callee specs
shown once per session (dedup), `spec` views except for the definitions the run then edited (spec-first), and
both. It regenerates every recorded view with `kern mcp` on the run's module (all views came before any edit) and
checks that the editor views come out identical to the recorded ones, then converts the saving to the frozen metric
with a per-token rate fitted on the runs' own read-only turns. Agent behaviour is held fixed: an agent shown less
might have asked for more.
"""
import collections
import glob
import json
import os
import shutil
import statistics as st
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, "..", "..", ".."))
C2_REV = "b43783a"           # git_rev in every runs/*/record.json
ARMS, TASKS = ("kview", "kfull", "ts"), ("T1", "T2", "T3")


def tokenizer():
    path = None
    if "--tokenizer" in sys.argv:
        path = sys.argv[sys.argv.index("--tokenizer") + 1]
    path = path or os.environ.get("KERN_TOKENIZER") or os.path.expanduser("~/.cache/kern/tokenizer.json")
    try:
        from tokenizers import Tokenizer
    except ImportError:
        print("(no `tokenizers` package: token counts skipped)\n")
        return None
    if not os.path.exists(path):
        print(f"(no tokenizer file at {path}: token counts skipped)\n")
        return None
    t = Tokenizer.from_file(path)
    return lambda s: len(t.encode(s).ids)


def runs():
    """{(arm, task, seed): (requests' usage in order, final usage, transcript messages)}"""
    out = {}
    for d in sorted(glob.glob(f"{HERE}/runs/*_T*_*")):
        arm, task, seed = os.path.basename(d).split("_")
        msgs, reqs = [], {}
        for line in open(f"{d}/workspace/transcript.jsonl"):
            try:
                m = json.loads(line)
            except ValueError:
                continue
            msgs.append(m)
            if m.get("type") == "assistant":
                reqs.setdefault(m["message"]["id"], m["message"]["usage"])   # one entry per API request
        final = json.load(open(f"{d}/workspace/final.json"))["usage"]
        out[(arm, task, int(seed))] = (list(reqs.values()), final, msgs)
    return out


def tool_results(msgs):
    """[(tool, text)] for every tool result in a transcript; kern_view is split by its `as` mode."""
    names, out = {}, []
    for m in msgs:
        content = m.get("message", {}).get("content")
        if not isinstance(content, list):
            continue
        for b in content:
            if b.get("type") == "tool_use":
                key = b["name"].split("__")[-1]
                if key == "kern_view":
                    key += ":" + b["input"].get("as", "iface")
                names[b["id"]] = (key, b["input"])
            elif b.get("type") == "tool_result":
                c = b.get("content")
                text = "".join(x.get("text", "") for x in c) if isinstance(c, list) else str(c)
                out.append((names[b["tool_use_id"]], text))
    return out


def schemas():
    exe = os.path.join(ROOT, "target", "release", "kern")
    exe = exe if os.path.exists(exe) else shutil.which("kern")
    if not exe:
        return None
    req = ('{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}\n'
           '{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}\n')
    out = subprocess.run([exe, "mcp"], input=req, capture_output=True, text=True).stdout
    for line in out.splitlines():
        m = json.loads(line)
        if m.get("id") == 2:
            return m["result"]["tools"]
    return None


def git_show(path):
    return subprocess.run(["git", "-C", ROOT, "show", f"{C2_REV}:{path}"], capture_output=True, text=True,
                          check=True).stdout


def main():
    n = tokenizer()
    data = runs()

    print("1. First request of each run: cache_read (the shared cached prefix) and cache_creation")
    for arm in ARMS:
        cr = sorted({data[k][0][0]["cache_read_input_tokens"] for k in data if k[0] == arm})
        cc = [data[k][0][0]["cache_creation_input_tokens"] for k in data if k[0] == arm]
        print(f"   {arm:6} cache_read {cr} (distinct values over {len(cc)} runs), cache_creation {min(cc)}-{max(cc)}")

    print("\n2. Frozen metric (input + cache_creation + output, final.json), medians: first request vs the rest")
    for task in TASKS:
        cells = []
        for arm in ARMS:
            ks = [(arm, task, s) for s in range(3)]
            tot = [data[k][1]["input_tokens"] + data[k][1]["cache_creation_input_tokens"]
                   + data[k][1]["output_tokens"] for k in ks]
            first = [data[k][0][0]["cache_creation_input_tokens"] for k in ks]
            rest = [t - f for t, f in zip(tot, first)]
            cells.append(f"{arm} total {st.median(tot):,.0f} = first {st.median(first):,.0f} + rest {st.median(rest):,.0f}")
        print(f"   {task}: " + " | ".join(cells))

    print("\n3. K-view tool use (runs that used it / calls)")
    used, calls = collections.Counter(), collections.Counter()
    for k in data:
        if k[0] == "kview":
            names = [t[0].split(":")[0] for t, _ in tool_results(data[k][2])]
            calls.update(names)
            used.update(set(names))
    for t in ("kern_list", "kern_view", "kern_check", "kern_callers", "kern_edit", "kern_add", "kern_why",
              "kern_run", "kern_replace", "kern_remove"):
        print(f"   {t:13} {used[t]}/9 runs, {calls[t]} calls")
    modes = collections.Counter(t[0] for k in data if k[0] == "kview" for t, _ in tool_results(data[k][2])
                                if t[0].startswith("kern_view"))
    print(f"   kern_view modes: {dict(modes)}")

    if n is None:
        return
    print("\n4. K-view tool results, proxy tokens")
    by, per_run = collections.defaultdict(list), {}
    for k in sorted(data):
        if k[0] != "kview":
            continue
        c = collections.Counter()
        for (tool, _), text in tool_results(data[k][2]):
            by[tool].append(n(text))
            c[tool] += n(text)
        per_run[k] = c
    for tool, v in sorted(by.items()):
        print(f"   {tool:18} {len(v):3} calls, {sum(v):6,} tokens, median {st.median(v):,.0f}")
    module = n(git_show("examples/escrow_s1.k"))
    for k, c in per_run.items():
        viewed = [i.get("name") for (t, i), _ in tool_results(data[k][2]) if t.startswith("kern_view")]
        print(f"   {k[1]} seed {k[2]}: editor views {c['kern_view:editor']:,} tokens ({', '.join(viewed)}); "
              f"whole escrow_s1.k {module:,}")

    print("\n5. Fixed text, proxy tokens")
    ref_then, ref_now = n(git_show("docs/REFERENCE.md")), n(open(os.path.join(ROOT, "docs", "REFERENCE.md")).read())
    print(f"   REFERENCE.md: {ref_then:,} at {C2_REV}, {ref_now:,} now")
    gap = st.median(data[k][0][0]["cache_creation_input_tokens"] for k in data if k[0] == "kview") - \
        st.median(data[k][0][0]["cache_creation_input_tokens"] for k in data if k[0] == "ts")
    print(f"   first-request cache_creation, K-view - TS (median): {gap:,.0f} real tokens for REFERENCE + preamble "
          f"difference, vs {ref_then:,} proxy for REFERENCE: real/proxy ~{gap / ref_then:.2f}")
    tools = schemas()
    if tools is None:
        print("   tool schemas: no kern binary found, skipped")
        return
    total = 0
    for t in tools:
        c = n(json.dumps(t, separators=(",", ":")))
        total += c
        print(f"   {t['name']:13} {c:4} (description {n(t['description'])})")
    print(f"   all ten tool schemas: {total} (as `kern mcp` serves them now; the Python server at {C2_REV} "
          f"had the same descriptions except kern_view's, reworded on 2026-11-05: 30 description tokens then, "
          f"and no `full`)")
    designs(n, data)


# ---- 6. Other view designs, simulated on the recorded runs (added 2026-11-05) -------------------------------------

def kern_exe():
    exe = os.path.join(ROOT, "target", "release", "kern")
    return exe if os.path.exists(exe) else shutil.which("kern")


def serve_views(requests, tok_path, full=True):
    """kern_view results from one `kern mcp` process (one session), one per (file, name, as) request. With `full`,
    every view is whole (as the server sent them during C2); without it, the server stubs repeated callee specs."""
    lines = ['{"jsonrpc":"2.0","id":0,"method":"initialize","params":{}}']
    for i, (f, name, as_) in enumerate(requests, 1):
        lines.append(json.dumps({"jsonrpc": "2.0", "id": i, "method": "tools/call", "params": {
            "name": "kern_view", "arguments": {"file": f, "name": name, "as": as_, "full": full}}}))
    env = dict(os.environ, KERN_TOKENIZER=tok_path)       # the "tokens" field in each view needs it
    out = subprocess.run([kern_exe(), "mcp"], input="\n".join(lines) + "\n", capture_output=True, text=True,
                         env=env).stdout
    got = {}
    for line in out.splitlines():
        m = json.loads(line)
        if m.get("id"):
            got[m["id"]] = m["result"]["content"][0]["text"]
    return [got[i] for i in range(1, len(requests) + 1)]


def view_json(text, n):
    return json.dumps({"text": text, "tokens": n(text)}, indent=1)


def dedup(results, n, shown=None):
    """Replace every callee spec block already shown earlier in the session by a one-line stub. A block counts as
    shown if the same text appeared before, or if that definition was the subject of an earlier view (its spec is
    in it). `results` is [(subject, view json)]; returns the new view jsons."""
    shown_text, shown_defs, out = set(), set(shown or ()), []
    for subject, r in results:
        blocks = json.loads(r)["text"].split("\n\n")
        kept = [blocks[0]]
        for b in blocks[1:]:
            name = b.split(" ", 1)[0]
            if b in shown_text or name in shown_defs:
                kept.append(f"{name} (spec shown above)")
            else:
                kept.append(b)
                shown_text.add(b)
        shown_defs.add(subject)
        out.append(view_json("\n\n".join(kept), n))
    return out


def fit_marginal(data, n):
    """Real tokens per proxy token of tool-result text: least squares of each request's cache_creation on the proxy
    tokens of the tool results it adds and the number of tool calls, over K-view requests whose previous turn only
    read (list/view/callers/check: no edit with a long source in its input)."""
    rows = []
    for k in sorted(data):
        if k[0] != "kview":
            continue
        order, calls, res, cur = [], collections.defaultdict(list), collections.Counter(), None
        for m in data[k][2]:
            if m.get("type") == "assistant":
                cur = m["message"]["id"]
                if cur not in order:
                    order.append(cur)
                calls[cur] += [b["name"] for b in m["message"]["content"] if b.get("type") == "tool_use"]
            elif m.get("type") == "user" and isinstance(m["message"]["content"], list):
                for b in m["message"]["content"]:
                    if b.get("type") == "tool_result":
                        c = b["content"]
                        res[cur] += n("".join(x.get("text", "") for x in c) if isinstance(c, list) else str(c))
        usage = {u_id: u for u_id, u in zip(order, data[k][0])}
        for prev, nxt in zip(order, order[1:]):
            if calls[prev] and all(c.split("__")[-1] in ("kern_list", "kern_view", "kern_callers", "kern_check")
                                   for c in calls[prev]):
                rows.append((res[prev], len(calls[prev]), usage[nxt]["cache_creation_input_tokens"]))
    # normal equations for y = a + b*x + c*z, solved by Gauss-Jordan
    X = [(1.0, x, z) for x, z, _ in rows]
    Y = [y for _, _, y in rows]
    A = [[sum(r[i] * r[j] for r in X) for j in range(3)] for i in range(3)]
    B = [sum(r[i] * y for r, y in zip(X, Y)) for i in range(3)]
    M = [A[i] + [B[i]] for i in range(3)]
    for i in range(3):
        p = max(range(i, 3), key=lambda r: abs(M[r][i]))
        M[i], M[p] = M[p], M[i]
        for r in range(3):
            if r != i:
                f = M[r][i] / M[i][i]
                M[r] = [a - f * b for a, b in zip(M[r], M[i])]
    a, b, c = (M[i][3] / M[i][i] for i in range(3))
    resid = max(abs(y - (a + b * x + c * z)) for x, z, y in rows)
    return b, (a, c, len(rows), resid)


def designs(n, data):
    tok_path = None
    if "--tokenizer" in sys.argv:
        tok_path = sys.argv[sys.argv.index("--tokenizer") + 1]
    tok_path = os.path.abspath(tok_path or os.environ.get("KERN_TOKENIZER")
                               or os.path.expanduser("~/.cache/kern/tokenizer.json"))
    if not kern_exe():
        print("\n6. no kern binary: view designs skipped")
        return
    import tempfile
    tmp = tempfile.mkdtemp(prefix="c2_views_")
    module = os.path.join(tmp, "escrow_s1.k")
    open(module, "w").write(git_show("examples/escrow_s1.k"))

    print("\n6. Other view designs, simulated on the K-view runs (proxy tokens of kern_view results); the (server) "
          "columns send the same calls through `kern mcp`'s own session dedup")
    b, (a, c, rows, resid) = fit_marginal(data, n)
    print(f"   calibration: each request's cache_creation = {a:.0f} + {b:.2f} x (proxy tokens of the tool results it "
          f"adds) + {c:.0f} x (tool calls), over {rows} read-only turns (largest residual {resid:.0f}). So one proxy "
          f"token of view text cost ~{b:.2f} real tokens under the frozen metric.")
    per_run, mismatches, first_editor = {}, 0, {}
    for k in sorted(data):
        if k[0] != "kview":
            continue
        views, targets, edited = [], set(), False
        for (tool, inp), text in tool_results(data[k][2]):
            if tool.startswith("kern_view"):
                assert not edited, f"{k}: a view after an edit; the module would have to be replayed"
                views.append((inp["name"], inp.get("as", "iface"), text))
            elif tool in ("kern_edit", "kern_replace", "kern_remove", "kern_add"):
                edited = True
                ops = inp.get("ops") or [dict(inp, op=tool.split("_")[1])]
                targets |= {o["name"] for o in ops if o.get("op") in ("replace", "remove") and o.get("name")}
        editor = serve_views([(module, v[0], "editor") for v in views], tok_path)
        spec = serve_views([(module, v[0], "spec") for v in views], tok_path)
        mismatches += sum(e != v[2] for e, v in zip(editor, views))
        recorded = [(v[0], v[2]) for v in views]
        spec_first = [(v[0], e if v[0] in targets else s) for v, e, s in zip(views, editor, spec)]
        # spec-first keeps editor views only for edit targets; "both" dedups callee blocks in those, with every spec
        # view shown earlier counting as shown
        both = []
        for i, (subj, r) in enumerate(spec_first):
            prior = {s for s, _ in spec_first[:i]}
            both.append(dedup(spec_first[:i + 1], n, shown=prior)[-1] if subj in targets else r)
        # the same sequences through the server's own session dedup (kern mcp since 2026-11-05)
        server_dedup = serve_views([(module, v[0], "editor") for v in views], tok_path, full=False)
        server_both = serve_views([(module, v[0], "editor" if v[0] in targets else "spec") for v in views], tok_path,
                                  full=False)
        lenient = [(v[0], e if (v[0] in targets or i == 0) else s) for i, (v, e, s) in enumerate(zip(views, editor, spec))]
        cost = lambda rs: sum(n(r) for r in rs)
        per_run[k] = {
            "recorded": cost(v[2] for v in views),
            "dedup": cost(dedup(recorded, n)),
            "spec-first": cost(r for _, r in spec_first),
            "both": cost(both),
            "both, 1st view editor": cost(dedup(lenient, n)),
            "dedup (server)": cost(server_dedup),
            "both (server)": cost(server_both),
        }
        first_editor[k] = sorted(targets)
    print(f"   regenerated editor views identical to the recorded ones: {sum(len([1 for (t, _), _ in tool_results(data[k][2]) if t.startswith('kern_view')]) for k in per_run) - mismatches} "
          f"of {sum(len([1 for (t, _), _ in tool_results(data[k][2]) if t.startswith('kern_view')]) for k in per_run)}")
    print("   edit targets (stay editor under spec-first): " +
          ", ".join(f"{k[1]}/{k[2]} {t or '-'}" for k, t in first_editor.items()))
    names = list(next(iter(per_run.values())))
    print("   view tokens per run (proxy), median over seeds:")
    for task in TASKS:
        ks = [("kview", task, s) for s in range(3)]
        print(f"     {task}: " + " | ".join(f"{d} {st.median(per_run[k][d] for k in ks):,.0f}" for d in names))

    print("   K-view total under the frozen metric = recorded total - ~%.2f x proxy saving; median; vs TS median" % b)
    ts_med = {t: st.median(data[("ts", t, s)][1]["input_tokens"] + data[("ts", t, s)][1]["cache_creation_input_tokens"]
                           + data[("ts", t, s)][1]["output_tokens"] for s in range(3)) for t in TASKS}
    ref_real = st.median(data[k][0][0]["cache_creation_input_tokens"] for k in data if k[0] == "kview") - \
        st.median(data[k][0][0]["cache_creation_input_tokens"] for k in data if k[0] == "ts")
    for task in TASKS:
        ks = [("kview", task, s) for s in range(3)]
        tot = {k: data[k][1]["input_tokens"] + data[k][1]["cache_creation_input_tokens"] + data[k][1]["output_tokens"]
               for k in ks}
        cells = []
        for d in names:
            med = st.median(tot[k] - b * (per_run[k]["recorded"] - per_run[k][d]) for k in ks)
            cells.append(f"{d} {med:,.0f} ({med / ts_med[task]:.2f}x)")
        best = min(st.median(tot[k] - b * (per_run[k]["recorded"] - per_run[k][d]) for k in ks) for d in names)
        need = best - 0.7 * ts_med[task]
        print(f"     {task} (TS {ts_med[task]:,.0f}, 0.7x = {0.7 * ts_med[task]:,.0f}): " + " | ".join(cells))
        ref_proxy = n(git_show("docs/REFERENCE.md"))
        print(f"        best design is {need:,.0f} real tokens over 0.7x; REFERENCE + preamble cost ~{ref_real:,.0f} "
              f"real in the first request" + (": even with no REFERENCE at all it stays over" if need > ref_real else
              f": REFERENCE would have to lose ~{need:,.0f} real tokens, i.e. shrink from {ref_proxy:,} to at most "
              f"~{ref_proxy - need / (ref_real / ref_proxy):,.0f} proxy tokens (if the preamble difference is ~0; "
              f"less if it isn't)"))
    shutil.rmtree(tmp, ignore_errors=True)


if __name__ == "__main__":
    main()

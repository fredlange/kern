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
          f"had the same descriptions)")


if __name__ == "__main__":
    main()

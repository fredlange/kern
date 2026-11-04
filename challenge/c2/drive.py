"""C2 driver (added 2026-11-04, after freezing; not part of the hashed material). Runs cells through the
frozen run.py and grade.py, keeps every artifact, and computes the protocol's metrics from raw usage
(grade.py folds cache reads into "input"; PROTOCOL.md says tokens = input + cache_creation + output,
with cache reads reported separately).

    .venv/bin/python challenge/c2/drive.py run ARM TASK SEED      one cell (needs approval: it spends money)
    .venv/bin/python challenge/c2/drive.py all [--parallel 3]     every cell not yet in runs/
    .venv/bin/python challenge/c2/drive.py report                 the table, per run and per cell
"""
import concurrent.futures
import json
import os
import re
import shutil
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(os.path.dirname(HERE))
PY = f"{ROOT}/.venv/bin/python"
RUNS = f"{HERE}/runs"
ARMS, TASKS, SEEDS = ("kview", "kfull", "ts"), ("T1", "T2", "T3"), (0, 1, 2)


def run(arm, task, seed):
    out = f"{RUNS}/{arm}_{task}_{seed}"
    if os.path.exists(f"{out}/grade.json"):
        return json.load(open(f"{out}/grade.json"))
    p = subprocess.run([PY, f"{HERE}/run.py", arm, task, str(seed), "--go"], capture_output=True, text=True, cwd=ROOT)
    rec = json.loads(p.stdout[p.stdout.index("{"):]) if "{" in p.stdout else {"error": p.stderr[-2000:]}
    ws = rec.get("workspace")
    os.makedirs(out, exist_ok=True)
    if ws and os.path.isdir(ws):
        shutil.copytree(ws, f"{out}/workspace", dirs_exist_ok=True)
    json.dump(rec, open(f"{out}/record.json", "w"), indent=1)
    args = [PY, f"{HERE}/grade.py", arm, task, f"{out}/workspace", "--transcript", f"{out}/workspace/final.json"]
    if arm == "kview":
        args += ["--mcp-log", f"{out}/workspace/mcp_log.jsonl"]
    g = subprocess.run(args, capture_output=True, text=True, cwd=ROOT)
    try:
        grade = json.loads(g.stdout)
    except ValueError:
        grade = {"grade_error": (g.stdout + g.stderr)[-2000:]}
    grade.update(metrics(arm, out))
    grade["valid"] = rec.get("valid")
    grade["chained_bash_executed"] = len(rec.get("chained_bash_executed", []))
    json.dump(grade, open(f"{out}/grade.json", "w"), indent=1)
    return grade


def metrics(arm, out):
    """Usage (all four numbers), cost, turns, budget hit, and check/test iterations from the transcript."""
    m = {}
    try:
        final = json.load(open(f"{out}/workspace/final.json"))
    except (OSError, ValueError):
        return {"metrics_error": "no final.json"}
    u = final.get("usage", {})
    four = {k: u.get(k, 0) for k in ("input_tokens", "cache_creation_input_tokens", "cache_read_input_tokens",
                                     "output_tokens")}
    m["usage"] = four
    m["tokens"] = four["input_tokens"] + four["cache_creation_input_tokens"] + four["output_tokens"]
    m["tokens_with_cache_reads"] = m["tokens"] + four["cache_read_input_tokens"]
    m["cost_usd"], m["turns"] = final.get("total_cost_usd"), final.get("num_turns")
    m["stop"] = final.get("subtype")                    # e.g. error_max_budget_usd, error_max_turns
    calls, results = [], {}
    for line in open(f"{out}/workspace/transcript.jsonl"):
        try:
            ev = json.loads(line)
        except ValueError:
            continue
        for b in (ev.get("message") or {}).get("content", []) if isinstance(ev.get("message"), dict) else []:
            if isinstance(b, dict) and b.get("type") == "tool_use":
                calls.append(b)
            if isinstance(b, dict) and b.get("type") == "tool_result":
                c = b.get("content")
                results[b.get("tool_use_id")] = json.dumps(c) if not isinstance(c, str) else c
    if arm == "kview":
        checks = [c for c in calls if c["name"] == "mcp__kern__kern_check"]
    elif arm == "kfull":
        checks = [c for c in calls if c["name"] == "Bash" and " -m kern check" in c["input"].get("command", "")]
    else:
        checks = [c for c in calls if c["name"] == "Bash" and "node --test" in c["input"].get("command", "")]
    refused = re.compile(r"requires? approval|haven't granted|was denied|expansion obfuscation")
    checks = [c for c in checks if not refused.search(results.get(c["id"], ""))]   # only checks that ran
    m["check_iterations"] = len(checks)
    if checks:
        first = results.get(checks[0]["id"], "")
        m["first_check_clean"] = _clean(arm, first)
    m["tool_calls"] = len(calls)
    return m


def _clean(arm, text):
    if arm == "ts":
        # node's default reporter prints "ℹ fail 0"; the TAP reporter "# fail 0"
        return bool(re.search(r"(?:#|ℹ) fail 0\b", text)) and not re.search(r"(?:#|ℹ) fail [1-9]", text)
    if arm == "kview":
        return "clean" in text.lower() and "not clean" not in text.lower()
    # K-full: a positive marker, every definition's status line is green (not just "no failure word")
    heads = re.findall(r"^\w+ \[[0-9a-f]+\]  (.+)$", text, re.M)
    return bool(heads) and "Exit code" not in text and \
        all(h.strip() == "All guarantees proven" or h.startswith("Proven, assuming") for h in heads)


def report():
    rows = []
    for arm in ARMS:
        for task in TASKS:
            for seed in SEEDS:
                p = f"{RUNS}/{arm}_{task}_{seed}/grade.json"
                if os.path.exists(p):
                    rows.append((arm, task, seed, json.load(open(p))))
    print(f"{'arm':6} {'task':4} {'s':2} {'valid':5} {'success':7} {'accept':8} {'silent breaks':28} "
          f"{'tokens':>8} {'+reads':>9} {'cost':>6} {'iters':>5} {'1st':>5} stop")
    for arm, task, seed, g in rows:
        print(f"{arm:6} {task:4} {seed:<2} {str(g.get('valid')):5} {str(g.get('success')):7} "
              f"{g.get('acceptance', '-'):8} {','.join(g.get('guarantees_broken_silently', []))[:28]:28} "
              f"{g.get('tokens', 0):>8} {g.get('tokens_with_cache_reads', 0):>9} {g.get('cost_usd') or 0:>6.2f} "
              f"{g.get('check_iterations', '-'):>5} {str(g.get('first_check_clean', '-')):>5} {g.get('stop', '')}")
    return rows


def remetric():
    """Recompute the transcript metrics of every graded run (after a fix to `metrics`/`_clean`)."""
    for d in sorted(os.listdir(RUNS)):
        p = f"{RUNS}/{d}/grade.json"
        if d != "void" and os.path.exists(p):
            g = json.load(open(p))
            g.update(metrics(d.split("_")[0], f"{RUNS}/{d}"))
            json.dump(g, open(p, "w"), indent=1)


if __name__ == "__main__":
    cmd = sys.argv[1]
    if cmd == "remetric":
        remetric()
    elif cmd == "run":
        print(json.dumps(run(sys.argv[2], sys.argv[3], int(sys.argv[4])), indent=1))
    elif cmd == "all":
        n = int(sys.argv[sys.argv.index("--parallel") + 1]) if "--parallel" in sys.argv else 3
        cells = [(a, t, s) for s in SEEDS for t in TASKS for a in ARMS]
        with concurrent.futures.ThreadPoolExecutor(n) as ex:
            for cell, g in zip(cells, ex.map(lambda c: run(*c), cells)):
                print(cell, g.get("success"), g.get("valid"), flush=True)
        report()
    else:
        report()

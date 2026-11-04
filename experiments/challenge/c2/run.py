"""C2 runner (frozen before any run). Dry-run by default: prints what would run. `--go` runs it, and
needs Fredrik's approval (27 headless agent runs, roughly 2-4M tokens; docs/CHALLENGE.md).

    .venv/bin/python challenge/c2/run.py ARM TASK SEED [--go]

Each run gets a fresh, isolated workspace with design history removed: no docs/ (except
REFERENCE.md as allowed), no CLAUDE.md, AGENTS.md, .why, out/, challenge/ or git history. Only the
arm's material is present, and only the arm's tools are allowed (checked from the transcript
afterwards by `verify`).
"""
import json
import os
import shutil
import subprocess
import sys
import tempfile

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
HERE = os.path.dirname(os.path.abspath(__file__))
PY = f"{ROOT}/.venv/bin/python"
MODEL = "claude-opus-5-5"
MAX_TURNS = 40
MAX_BUDGET_USD = 4.0         # per run; a run that hits it ends there and is graded as it stands
KVIEW = ["mcp__kern__kern_list", "mcp__kern__kern_view", "mcp__kern__kern_callers", "mcp__kern__kern_why",
         "mcp__kern__kern_check", "mcp__kern__kern_run", "mcp__kern__kern_edit", "mcp__kern__kern_add",
         "mcp__kern__kern_replace", "mcp__kern__kern_remove"]


# the built-in tools that exist at all (--tools); --allowedTools only pre-approves. --restricted ignores
# user/project settings (hooks, plugins) and confines file tools to the workspace (added 2026-11-04,
# before any run: a plain `claude -p` loaded ~27k tokens of the operator's own setup)
BUILTIN = {"kview": "", "kfull": "Read,Edit,Write,Bash", "ts": "Read,Edit,Write,Bash"}


def tools(arm, ws):
    """Allowed tools per arm, file tools scoped to the workspace (kern-90: Read takes absolute paths)."""
    # absolute paths in permission rules take a double slash ("//abs/path"); "/abs" never matched, so in
    # the first 18 K-full/TS runs every Edit/Write was refused (found 2026-11-04 after those runs)
    scoped = [f"Read(/{ws}/**)", f"Edit(/{ws}/**)", f"Write(/{ws}/**)"]
    return {"kview": KVIEW, "kfull": scoped + [f"Bash({PY} -m kern:*)"], "ts": scoped + ["Bash(node --test:*)"]}[arm]


def preamble(arm, ws):
    ref = open(f"{ROOT}/docs/REFERENCE.md").read()
    if arm == "kview":
        return (f"You maintain a module written in Kern. Kern's language reference is below. You can only work through "
                f"the kern tools (kern_list, kern_view, kern_callers, kern_check, kern_edit, ...); there is no file "
                f"access. The module is {ws}/escrow_s1.k. Make the change below, and finish when kern_check reports the "
                f"module clean. Don't ask questions.\n\n{ref}\n\n## The change\n\n")
    if arm == "kfull":
        return (f"You maintain a module written in Kern. Kern's language reference is below. The module is "
                f"{ws}/escrow_s1.k; check it with `{PY} -m kern check {ws}/escrow_s1.k --quick`. Make the change below, "
                f"and finish when the check is clean. Don't ask questions.\n\n{ref}\n\n## The change\n\n")
    return (f"You maintain the TypeScript module {ws}/ts/escrow.ts (tests: `node --test {ws}/ts/escrow.test.ts`). "
            f"Make the change below, and finish when the tests pass. Don't ask questions.\n\n## The change\n\n")


def task_text(task):
    text = open(f"{HERE}/tasks.md").read()
    part = text.split(f"## {task}:", 1)[1].split("\n## ", 1)[0]
    return part.split("\n", 1)[1].strip()


def workspace(arm):
    ws = os.path.realpath(tempfile.mkdtemp(prefix=f"c2_{arm}_"))   # /var/folders is a symlink to /private/var
    if arm == "ts":
        os.makedirs(f"{ws}/ts")
        for f in ("escrow.ts", "platform.ts", "escrow.test.ts"):
            shutil.copy(f"{ROOT}/compare/ts/{f}", f"{ws}/ts/{f}")
    else:
        shutil.copy(f"{ROOT}/examples/escrow_s1.k", f"{ws}/escrow_s1.k")
    return ws


def command(arm, task, ws, seed):
    mcp = f"{ws}/.mcp.json"
    with open(mcp, "w") as f:
        # claude ignores "cwd" here (the server started in the workspace and failed to import kern,
        # found 2026-11-04 before any run), so the package path goes in through PYTHONPATH
        servers = {"kern": {"command": PY, "args": ["-m", "kern.mcp"],
                            "env": {"PYTHONPATH": ROOT, "KERN_MCP_LOG": f"{ws}/mcp_log.jsonl",
                                    "KERN_MCP_ROOT": ws}}} if arm == "kview" else {}
        json.dump({"mcpServers": servers}, f)
    prompt = preamble(arm, ws) + task_text(task)
    return ["claude", "-p", prompt, "--model", MODEL, "--max-turns", str(MAX_TURNS), "--max-budget-usd",
            str(MAX_BUDGET_USD), "--output-format", "stream-json", "--verbose", "--mcp-config", mcp,
            "--strict-mcp-config", "--restricted", "--tools", BUILTIN[arm],
            "--allowedTools", ",".join(tools(arm, ws))], prompt


def verify(arm, transcript_lines, ws):
    """Every tool the agent used is one its arm allows, and no tool call (refused or not) names a path outside
    the workspace. Chained Bash commands that ran are listed, not invalidating (narrowed 2026-11-04, see
    PROTOCOL.md). A run that fails this is INVALID: replaced and counted."""
    import re
    used, outside, chained, executed_chained = [], [], {}, []
    for line in transcript_lines:
        try:
            ev = json.loads(line)
        except ValueError:
            continue
        for block in (ev.get("message") or {}).get("content", []) if isinstance(ev.get("message"), dict) else []:
            if isinstance(block, dict) and block.get("type") == "tool_use":
                used.append(block["name"])
                text = json.dumps(block.get("input", {}))
                # absolute paths only: `ts/escrow.ts` used to match as "/escrow.ts" (fixed 2026-11-04)
                # ... and a path has a name after its first slash: `// comment` in code isn't one (fixed 2026-11-04)
                for path in re.findall(r"(?:^|(?<=[\s\"'=]))(/[\w.-][\w./-]*)", text):
                    if not os.path.realpath(path).startswith(os.path.realpath(ws)) and not path.startswith(PY):
                        outside.append(path)
                if block["name"] == "Bash" and re.search(r"[;&|`]|\$\(", block.get("input", {}).get("command", "")):
                    chained[block["id"]] = block["input"]["command"]
            if isinstance(block, dict) and block.get("type") == "tool_result" and block.get("tool_use_id") in chained:
                # narrowed 2026-11-04 (PROTOCOL.md): a chained command claude refused is harmless; one that ran
                # had every part pre-approved or read-only inside the workspace. The outside-path check above
                # applies to every call, refused or not
                if not re.search(r"requires? approval|haven't granted|was denied", json.dumps(block.get("content"))):
                    executed_chained.append(chained[block["tool_use_id"]][:200])
    allowed = {t.split("(")[0] for t in tools(arm, ws)}
    bad = sorted(set(u for u in used if u not in allowed))
    return {"tools_used": used, "outside_arm": bad, "outside_workspace": sorted(set(outside)),
            "chained_bash_executed": executed_chained, "valid": not bad and not outside}


def main():
    arm, task, seed = sys.argv[1], sys.argv[2], int(sys.argv[3])
    ws = workspace(arm)
    cmd, prompt = command(arm, task, ws, seed)
    sys.path.insert(0, ROOT)
    from kern.report import tool_hash             # which tools this run used (the MCP server imports ROOT)
    rev = subprocess.run(["git", "rev-parse", "HEAD"], cwd=ROOT, capture_output=True, text=True).stdout.strip()
    dirty = bool(subprocess.run(["git", "status", "--porcelain", "kern"], cwd=ROOT, capture_output=True,
                                text=True).stdout.strip())
    record = {"arm": arm, "task": task, "seed": seed, "model": MODEL, "max_turns": MAX_TURNS, "workspace": ws,
              "tool_hash": tool_hash(), "git_rev": rev, "kern_dirty": dirty,
              "max_budget_usd": MAX_BUDGET_USD, "tools": tools(arm, ws), "prompt_sha256": __import__("hashlib").sha256(prompt.encode()).hexdigest()}
    if "--go" not in sys.argv:
        print(json.dumps(dict(record, dry_run=True, command=cmd[:2] + ["<prompt>"] + cmd[3:]), indent=1))
        return
    # the agent's Bash needs to import kern from the workspace (`python -m kern check`); found 2026-11-04
    p = subprocess.run(cmd, capture_output=True, text=True, cwd=ws, env=dict(os.environ, PYTHONPATH=ROOT))
    lines = p.stdout.splitlines()
    open(f"{ws}/transcript.jsonl", "w").write(p.stdout)
    final = json.loads(lines[-1]) if lines else {}
    json.dump(final, open(f"{ws}/final.json", "w"))
    record.update(verify(arm, lines, ws), exit=p.returncode)
    json.dump(record, open(f"{ws}/record.json", "w"), indent=1)
    print(json.dumps(record, indent=1))


if __name__ == "__main__":
    main()

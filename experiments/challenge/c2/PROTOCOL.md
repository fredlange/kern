# C2 protocol (prepared 2026-11-04; thresholds frozen 2026-11-04; NOT RUN yet)

C2 tests the central claim with real tasks: does an agent working through Kern's views do as well
as one reading TypeScript, for fewer tokens, without breaking guarantees it never read? See
docs/CHALLENGE.md, C2.

**Status:** everything below is prepared and validated without agent runs. Running it needs
Fredrik's approval (27 runs, roughly 2–4M tokens), and Fredrik sets the final thresholds when
freezing (rule 5). Until then, CHALLENGE.md's proposed defaults stand as proposals.

**2026-11-04: approved and frozen by Fredrik.** The run is approved, and the thresholds are
CHALLENGE.md's proposed ones, verbatim:
- K-view success ≥ TypeScript's success − 10 points;
- K-view breaks guarantees no more often than TypeScript; on T3 the target is 0 of 3;
- K-view tokens ≤ 0.7× TypeScript's (median per task; tokens = input + cache_creation + output);
- ≥ 50% of Kern runs pass `check` on the first try;
- Kern check iterations ≤ 2× TypeScript's test iterations.

N = 3 per cell makes this a smoke test: it can catch a large failure, not prove a win. The run
waits until the loop/handler fuzzer is done, so the tools don't change during it. Each record
holds the tool hash (`kern.report.tool_hash()`) and the git revision with a dirty flag. The
`HIDDEN.sha256.json` hashes were re-verified (10/10) at freezing, not rehashed.

**2026-11-04, changes to `run.py` before any run (all found by probing the command line, at
about $0.25 in total):**
- **The K-view arm had no tools.** Claude Code ignores the `cwd` key in an MCP config, so the kern
  server started in the workspace, couldn't import `kern`, and showed `status: failed`. Every
  K-view run would have run with no tools at all. Now `PYTHONPATH` goes in the server's env, and
  the probe shows `connected` with exactly the ten `mcp__kern__*` tools.
- **`--allowedTools` only pre-approves, it doesn't restrict.** Each arm now also passes
  `--tools` with its built-in set: K-view none, the others `Read,Edit,Write,Bash`.
- **`--restricted`:** a plain `claude -p` loaded about 27.5k tokens of the operator's own setup
  (plugins, skills, hooks). Restricted mode ignores user and project settings and confines the
  file tools to the workspace. The "say ok" overhead fell to ~2–5k tokens. That's the same for
  every arm, but it is the operator's setup, not the task's.
- **Each record holds** `tool_hash`, `git_rev` and `kern_dirty`, so it shows which tools each run
  used.
- `HIDDEN.sha256.json`: only `run.py` is rehashed, for these changes. The other 9 hashes are
  unchanged and were re-verified.

## Material

- **Tasks:** `tasks.md`, T1–T3, given verbatim after each arm's preamble (`run.py`).
  - T3 now says explicitly that every other operation keeps its current behaviour (kern-90).
    `tip A A 0` succeeding is part of what the agent must preserve; T3 exists to see whether
    the agent notices.
- **Arms** (`run.py`, the same model `claude-opus-5-5`, `--max-turns 40`, `--strict-mcp-config`):

  | arm | the agent gets | tools allowed (nothing else) |
  |---|---|---|
  | K-view | REFERENCE.md in the prompt, plus the kern MCP server | `mcp__kern__*` only (`--tools ""`): no Read, Bash, Grep or Glob |
  | K-full | REFERENCE.md, plus `escrow_s1.k` in its workspace | Read, Edit, Write, `Bash(python -m kern …)` |
  | TS | `ts/escrow.ts`, `platform.ts`, `escrow.test.ts` | Read, Edit, Write, `Bash(node --test …)` |

- **Workspaces** are fresh temp directories with only the arm's files: no docs/, CLAUDE.md,
  AGENTS.md, .why, out/, challenge/ or git history.
- **Isolation** (kern-90's audit closed this hole: `Read` takes absolute paths):
  - Read, Edit and Write are scoped to the workspace (`Read(<ws>/**)` …).
  - The K-view MCP server gets `KERN_MCP_ROOT=<ws>` and refuses any module outside it.
- **The validity rule, applied after each run.** `run.py` checks the stream-json transcript.
  A run is **INVALID**, then replaced and counted, if any tool call:
  - uses a tool outside its arm;
  - names a path outside the workspace;
  - is a Bash command that chains commands (`;`, `&`, `|`, `` ` ``, `$(`).

  It's untested whether `Bash(python -m kern:*)` and `Bash(node --test:*)` refuse chained
  commands, so this rule covers that case either way.
- **Budget:** `--max-budget-usd 4` per run. A run that hits it ends there and is graded as it
  stands; the hit is reported.

## Grading (`grade.py`, scripts only)

- **Success:** all 600 hidden acceptance cases of the task match (outcome with refusal
  arguments, balances, exact events).
  - The expected results come from reference solutions (`hidden/reference/`), whose Kern and
    TypeScript versions agree on every case.
- **Guarantees broken:** the *original* escrow_s1 specs (keep, ensure, emit, ok otherwise),
  checked on 1,000 random calls to the original definitions, outside the inputs the task
  legitimately changes (`exempt()`). The same check runs on Kern and TypeScript results.
- **Visible:** for a Kern result, whether the spec of the definition whose guarantee broke was
  changed, so the ratchet would show it. TypeScript has no spec, so its breaks are never visible.
- **Tokens:** reported as all four `claude -p` usage numbers: input, cache_creation, cache_read
  and output. **"Tokens"** for the 0.7× condition means **input + cache_creation + output**.
  Cache reads dominate and are re-reads of the same prefix, so they're reported separately and
  also as a second total.
- **Kern only:** whether the final module is check-clean; how many `kern_check` / `kern check`
  calls it took; whether the first was clean (from the MCP log, or from the Bash calls in the
  transcript).
- **Reporting:** with N = 3 runs per cell, differences of one run are not interpreted. Every
  run is reported individually, not only medians.

## Validation, without agents (`grade.py --validate`)

| | T1 | T2 | T3 |
|---|---|---|---|
| reference (Kern and TS) | 600/600, success | 600/600, success | 600/600, success, no breaks |
| untouched | 271/600 | 561/600 | 554/600 |
| T3 "move only" (callers untouched) | | | 564/600; tip's `ok otherwise` broken: Kern *not visible*, TS no spec |

The Kern references are behaviour references: they change bodies without updating every spec,
so they aren't check-clean. That doesn't affect the hidden cases, which test behaviour.

## Frozen hashes

`HIDDEN.sha256.json` holds the SHA-256 of every hidden or grading file. Verify them before any
run.

## C2 results

### Read this first: harness changes made after 9 graded runs (2026-11-04)

The first pass ran all 27 cells. The 9 K-view runs were valid and graded. **All 18 K-full and TS
runs were void because of harness faults,** none of which K-view used:
1. **Every Edit and Write was refused.** The rules were `Edit(/abs/path/**)`, but absolute paths
   in permission rules take a double slash (`Edit(//abs/path/**)`). The agents asked for
   permission and stopped. None worked around it: no file in any of the 18 workspaces changed.
2. **`python -m kern check` failed in the workspace** ("No module named kern"). It's the import
   problem already fixed for the K-view MCP server, but not for the agent's Bash.
3. **`verify()` misread relative paths:** `ts/escrow.ts` matched as `/escrow.ts`, "outside the
   workspace".
4. **`drive.py` scored every K-full first check as clean,** including the import failures. It now
   needs a positive marker: every definition's status line green.

**Fixes:**
- `run.py`: `//` rules on the real path of the workspace, and `PYTHONPATH` for the agent's
  process. Each was probed alone first: an edit inside lands, an edit outside is refused, and
  the check runs.
- `verify()`: matches only absolute paths.
- `drive.py`: the positive clean marker.
- `run.py` is rehashed. Grading, hidden cases, tasks, prompts and thresholds are unchanged.

**One frozen rule narrowed:** "a Bash command that chains commands makes a run invalid". Its stated
reason was that it was *untested* whether Claude Code refuses chained parts outside the
allowed set. The 18 void transcripts settle that: 54 chained commands, 18 refused by Claude Code,
36 run. Every part that ran was read-only or allowed, inside the workspace, and no part naming
a path outside it ran. So a chained command that runs no longer invalidates a run (each is listed
in `chained_bash_executed`). The outside-tool and outside-path rules stay strict, for refused
calls too. **If Fredrik prefers the rule as frozen, only the K-full and TS cells need rerunning.**
The 9 K-view runs had no chained commands, so they're valid under either version.

**Replaced and counted:** 18 runs replaced, all for harness faults, none for agent behaviour.
The void runs are kept in `runs/void/`.

**Also after the fixes:** with Edit permission on the workspace, Claude Code also lets a shell
write inside it run (`cat >> escrow_s1.k <<EOF … && kern check`, K-full T1 seed 0). That's the
Write tool's capability, inside the workspace, so it stays within the arm. `drive.py`'s TS
first-check marker missed node's default reporter (`ℹ fail 0`); it was fixed, and every run's
metrics were recomputed from its transcript (`drive.py remetric`).
- **Check iterations count only checks that ran.** A check inside a command Claude Code refused was never executed, so it is not counted (all arms).
- **`verify()` false positive, fixed after the reruns:** TS T3 seed 2 was marked invalid for the
  "path" `//`, which is a TypeScript comment (`// Keep tip's existing behaviour…`) in an Edit's
  new text. A path must now have a name after its first slash. All 27 records were re-verified
  with the fixed rule; only that run's validity changed (invalid → valid). `run.py` rehashed.

### Results (27 valid runs; claude-opus-5-5; tool hash `b808c9dd…`; raw data `results.json`, `runs/`)

Every run: success 600/600, no original guarantee broken, Kern modules check-clean at the end.

| arm | T1 tokens (median) | T2 | T3 | output tokens (median, T1/T2/T3) | first check clean | check iterations (median) | cost per run |
|---|---|---|---|---|---|---|---|
| K-view | 12,840 | 15,695 | 16,429 | 2,443 / 3,397 / 2,742 | 9/9 | 1 / 1 / 1 | $0.13–0.18 |
| K-full | 13,286 | 18,670 | 14,680 | 2,147 / 3,980 / 2,044 | 8/9 | 2 / 1 / 1 | $0.14–0.22 |
| TS | 12,474 | 13,209 | 12,395 | 3,343 / 3,337 / 3,274 | 9/9 | 1 / 1 / 1 | $0.11–0.18 |

Tokens = input + cache_creation + output. Cache reads are reported per run (`drive.py report`).

**Against the frozen thresholds:**
1. **K-view success ≥ TS − 10 points: pass** (100% vs 100%).
2. **K-view breaks guarantees no more often than TS, and T3 is 0/3: pass** (0 vs 0; T3 0/3).
3. **K-view tokens ≤ 0.7× TS (median per task): fail.** The ratio is 1.03 for T1, 1.19 for T2
   and 1.33 for T3. With cache reads it's 1.06, 0.97 and 0.99.
4. **≥ 50% of Kern runs pass check on the first try: pass** (17/18: K-view 9/9, K-full 8/9).
5. **Kern check iterations ≤ 2× TS test iterations: pass** (medians 1–2 against 1).

**What the numbers say, and don't:**
- **The token claim fails here, and the cause is visible.** Cache_creation (the prompt and tool
  definitions written once) is ~10.4–13.7k for K-view against ~9.1–9.4k for TS. The Kern arms
  carry REFERENCE.md (2,126 tokens) and, for K-view, ten MCP tool schemas. K-view *writes* less
  than TS (output 2.4–3.4k vs ~3.3k), but on a 10-definition module the fixed language tax
  outweighs anything the views save. That matches COMPARE.md's token proxies on escrow_s1.
  It's a result about a *small* module; nothing here says what happens at scale.
- **T3 did not discriminate.** Since kern-90's review, T3's text says "every other operation must
  keep its current behaviour exactly". TS's tests don't cover `tip A A 0`, so all three TS agents
  kept it from the text alone. All three K-view agents did it by guarding `tip`'s call. So T3 now
  measures instruction-following, not "notices a guarantee it never read". A trap that isn't
  announced is C3's job.
- **K-view vs K-full:** K-view is 0.97 / 0.84 / 1.12× K-full. Its first check was clean in 9/9,
  against 8/9.
- **N = 3 per cell:** every cell is 3/3, so the success rates are not distinguishable. At this
  size, every arm solved every task.
- **The model was the same family that designed Kern,** with the reference in its prompt. "Cold
  author" means no training data on Kern, not no exposure to its design style.

**Fredrik's decision (2026-11-04): the narrowed chained-Bash rule is accepted.** The 27 runs stand
as reported. (Correction to the question as first put to him: under the frozen rule, 15 of the 18
K-full/TS runs would have been invalid, not 17. Three K-full runs ran no chained command.)

**2026-11-04, layout note (after the run).** The repository was reorganised: this directory is now
`experiments/challenge/c2/`, `compare/` is `experiments/compare/`, and `kern/fuzz.py` and `kern/scorecard.py`
left `kern/` (so `tool_hash()` covers a different file set). The hashed files are unchanged and still
match `HIDDEN.sha256.json`, but `run.py` and `common.py` find the repository from their own location
and expect the old layout. A rerun needs a checkout of `d521236` (the run's commit) or a re-freeze.

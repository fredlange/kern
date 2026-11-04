# C2 protocol (prepared 2026-11-04; NOT RUN, and the thresholds are not frozen)

C2 tests the central claim with real tasks: does an agent working through Kern's views do as well
as one reading TypeScript, for fewer tokens, without breaking guarantees it never read? See
docs/CHALLENGE.md, C2.

**Status:** everything below is prepared and validated without agent runs. Running it needs
Fredrik's approval (27 runs, roughly 2–4M tokens), and Fredrik sets the final thresholds when
freezing (rule 5). Until then, CHALLENGE.md's proposed defaults stand as proposals.

## Material

- **Tasks:** `tasks.md`, T1–T3, given verbatim after each arm's preamble (`run.py`).
  - T3 now says explicitly that every other operation keeps its current behaviour (kern-90).
    `tip A A 0` succeeding is part of what the agent must preserve; T3 exists to see whether
    the agent notices.
- **Arms** (`run.py`, the same model `claude-opus-5-5`, `--max-turns 40`, `--strict-mcp-config`):

  | arm | the agent gets | tools allowed (nothing else) |
  |---|---|---|
  | K-view | REFERENCE.md in the prompt, plus the kern MCP server | `mcp__kern__*` only: no Read, Bash, Grep or Glob |
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

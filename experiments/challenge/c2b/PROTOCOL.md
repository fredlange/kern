# C2b protocol (DRAFT, 2026-11-05: not frozen, not approved, not run)

C2b asks C2's question again on a larger module (5.5× the definitions, 3.9× the tokens): does an agent
working through Kern's views do as well as one reading TypeScript, for fewer tokens, without breaking guarantees
it never read? C2 failed its token threshold on the 10-definition escrow (1.03 / 1.19 / 1.33× TS against
≤ 0.7×). Its after-the-run notes (`../c2/PROTOCOL.md`, three notes dated 2026-11-05) show that no view design
reaches 0.7× on a module that small. The fixed costs (REFERENCE.md, the session's own edits and checks) are larger
than anything views can save there. Views should save more as the module grows; C2b tests that
(docs/research/SHORTCOMINGS.md, S1).

**Status.** This is a draft written by agents (kern-9b, at kern-8b's request). Nothing here has been run.
- **Decided by Fredrik:** whether C2b runs at all (it costs agent runs), N, the module, and the final thresholds,
  which he freezes before any run (rule 5, as in C2).
- **Must exist before freezing** (no agent runs of the experiment; see "Prerequisites"): a TypeScript port of
  the module, hidden cases, a grader and a runner on the Rust toolchain.
- **The prediction below is written before any run** and will not be edited after one. It will be refreshed once,
  with the TS port's real size and the freeze commit's binary, before freezing. Both versions will be kept.

## Module: `examples/escrow/escrow_s3.k`

55 definitions, 775 lines, 8,828 proxy tokens, against 2,267 for C2's `escrow_s1.k` (3.89×).
- **It contains C2's module.** s3 holds s1's ten definitions (transfer, move, reserve, release, capture, purchase,
  cancel, refund, split, tip), mostly rewritten with `need as`/`emit as`. So C2's three tasks carry over with
  only the changes listed under "Tasks". What changes between C2 and C2b is mainly the size of the module
  and how far a change reaches.
- **The neighbourhoods grow, not only the file.** In s1, `move` has 2 callers. In s3 it has 5 direct and 7
  transitive callers. The fee rule reaches 14 definitions, and `capture` has 13 transitive callers.
- **Why not `ledger-large`:** it has 7 definitions in 82 lines, which is smaller than s1.
- s3 uses things s1 doesn't: `state` maps, `invariant`, `need as`/`emit as`, `distinct` and pure `-> Int`
  functions. REFERENCE.md documents every one of them (checked at f4f6674; df332d5 only added a bullet), so
  the Kern arms are not given a language they haven't been shown.

## Tasks

Given verbatim after the arm's preamble, one task per run. Each keeps C2's shape except T2, as noted there. The
changes from C2's text are listed under each task. The "reference change" lines (T2, T3) were verified on
scratch copies of s3 with `kern check --quick` and `kern run`, at f4f6674 and again at df332d5 (same results).

### T1: a new operation

> Add an operation `settle escrow buyer seller house amount part`. The escrow account holds `amount` for an order:
> - Pay `part` of it to the seller exactly as `capture escrow seller house part` does (same fee, same refusals,
>   same events).
> - Then release the remaining `amount - part` to the buyer exactly as `release escrow buyer (amount - part)`
>   does.
> - When `part = amount`, nothing is released (no release events).
> - Refuse with `BadPart` unless `0 < part <= amount`. That check comes first; after it, refusals are whatever
>   capture, then release, would give.
>
> The audit events must be exactly capture's events followed by release's events.

Changed from C2: `capture` has no `fee` argument in s3 (the fee is `fee_for amount`), so neither does `settle`.

### T2: a rule change in a callee

> From now on, the fee the house takes in `capture` is capped at 50: whenever the fee would be above 50, the house
> gets 50 and the seller gets `amount - 50`. Every operation that charges this fee must stay correct with the new
> rule, and so must its audit events. Nothing else changes.

Changed from C2: in s3 the fee is computed by the pure function `fee_for` (above 50 from `amount = 4100`), not
passed in.
- **Reference change:** `fee_for` alone (one more `ensure` and one more branch). `check` stays clean. Its callers'
  specs refer to `fee_for` by name, so they follow the change.
- **This is a different shape from C2's T2,** where `purchase` had to change too. s3 keeps the fee rule in one
  place, so the change is one definition. The work that grows with the module is *making sure* the 14 dependants
  are still right. In K-view that is one `kern_check`; in TS it is reading code or running tests.
- The variant that keeps C2's shape, a cap in `capture` only, is behaviourally identical: `fee_for` has no other
  caller in a body. So it can't be given as a separate task, and an agent may implement either.

### T3: a change with consequences

> `move` must refuse with `SameAccount` whenever `from = to`, even when `amount = 0`. (Today moving 0 to the same
> account succeeds and does nothing.) Every other operation must keep its current behaviour exactly.

Same text as C2.
- **Reference change:** `move`, plus a guard in `tip` and in `charge_late`. Both have `need as move`, so with
  `move` changed alone, **`check` stays clean** and both silently change behaviour: `tip A A 0` and
  `charge_late 2 A A 0` go from `ok` to `SameAccount` (measured with `kern run`).
- The other three direct callers, `split`, `checkout_vat` and `payout_holdback`, already refuse `from = to`.
- In C2 one caller needed the guard (`tip`); here two do. As in C2, the requirement is stated in the task text
  ("every other operation…"), so T3 measures whether the agent finds every affected caller, not whether it
  notices an unannounced trap (that is C3's job).

## Arms

The model is the same in every arm (C2's: `claude-opus-5-5`), with `--max-turns 40`, `--strict-mcp-config`,
`--restricted` and `--max-budget-usd 4`.

| arm | the agent gets | tools (nothing else) |
|---|---|---|
| K-view | REFERENCE.md in the prompt; `kern mcp` on `escrow_s3.k` | `mcp__kern__*` only (`--tools ""`) |
| K-full | REFERENCE.md; `escrow_s3.k` in the workspace | Read, Edit, Write, `Bash(kern check:*)` |
| TS | `escrow.ts` (the s3 port), `platform.ts`, `escrow.test.ts` | Read, Edit, Write, `Bash(node --test:*)` |

- **K-view's server** is the Rust `kern mcp` at the freeze commit, at or after 8d2a3d1. It includes the session
  dedup (a callee spec already shown comes back as a one-line stub) and `kern_view`'s description ("read with
  `spec`; `editor` only for a definition you will change"). Claude Code starts one server per run, so one
  `Session` covers the run. Each record must show exactly one `initialize`.
- **K-full stays.** It is the control that separates "views" from "Kern the language", and it is roadmap step 3's
  own comparison (editor view against full source). It costs about $10 of the estimated ~$17 (see "Cost"), so
  it is the arm to drop if Fredrik wants C2b cheaper.
  - K-full reads `kern check`'s CLI output. On s3 that is 12,848 proxy tokens per check (at df332d5), against 609 for
    `kern_check` through MCP. The CLI's report isn't designed for agents, and that asymmetry drives K-full's
    predicted cost.
  - As in C2, K-full gets plain `kern check` (no `--quick`). Its check output is reported as its own line.
- **No Grep or Glob for TS.** The tool sets are C2's, for continuity.
  - Under the narrowed chained-Bash rule, read-only shell commands ran in C2: the TS agents read files with
    `cd … && cat`. So TS agents can already search and read parts of files, and C2b doesn't take that away.

## N = 5 per cell (45 runs)

- **C2 used N = 3, and its cells spread widely.** The largest minus the smallest run, over the median, was 29%
  and 30% for TS on T1 and T3, and 43% for K-full on T1. With the prediction straddling 0.7×, a median of three
  decides almost nothing.
- **Five per cell is cheap** (see "Cost"), so the budget isn't what limits N.
- **N = 5 is still a smoke test** for success rates. One run is 20 points in a cell, or 6.7 points over an arm's
  15 runs.

## Thresholds (proposed; Fredrik freezes them before any run)

C2's five thresholds. Two things are new and need Fredrik's decision: the T3 target follows N (0 of 5), and
threshold 3 says outright that all three tasks must pass (C2's "median per task" left that open; C2 failed
all three, so it didn't matter there):
1. K-view success ≥ TS's success − 10 points (over each arm's 15 runs).
2. K-view breaks guarantees no more often than TS. On T3 the target is 0 of 5.
3. **K-view tokens ≤ 0.7× TS's**, as the median per task, under C2's frozen metric (input + cache_creation +
   output, cache reads excluded). This passes only if it holds on all three tasks. Each task is reported on its
   own.
4. ≥ 50% of Kern runs pass `check` on the first try.
5. Kern check iterations ≤ 2× TS's test iterations.

**Why 0.7× stays.**
- **Continuity:** C2b exists to see whether C2's failure was about module size. That needs C2's bar.
- **The prediction straddles it** (below), so the run can fail. That is what makes it a test.
- **Changing the bar after C2 failed it would look like moving the goalposts,** whatever the reason given.

**Reported, without a threshold:**
- K-view / K-full;
- the ratio with cache reads included;
- how much of K-view's tokens went to `kern_list`;
- which view modes the agents asked for (did they follow the description and read with `spec`?);
- how many callee specs the server stubbed;
- the guarantees broken that were visible on the proof page.

## Prediction (written 2026-11-05, before any C2b run)

`predict.py` (this directory) replays C2's 27 recorded runs as if the module had been s3.
- **Behaviour held fixed:** each agent makes the same calls in the same order, plus the extra reading the larger
  neighbourhoods force (T2 adds a view of `fee_for`; T3 adds views of `charge_late`, `checkout_vat` and
  `payout_holdback`, and one more edit).
- **How each tool result changes:** each one is replaced by what the same call returns on s3, measured with
  `kern` at df332d5 (`kern_list` 410 → 2,235 proxy tokens; `kern_check` 111 → 609; CLI `kern check` 1,835 →
  12,848).
- **For TS,** whole-file reads grow by an assumed 3.89× (code) and 3.81× (tests); no TS port of s3 exists yet.
- **Pricing:** C2's calibration, 1.65 real tokens per proxy token and 126 per tool call.
- **The commit:** every number here was measured at **df332d5**: `kern` built from a clean `git archive` of it
  (`KERN_BIN`), with REFERENCE.md and escrow_s3.k as of that commit.
  - The first draft used f4f6674. S8 (df332d5) moved two inputs. The CLI check report on s3 went from 10,234
    to 12,848 proxy tokens, grouping survivors per need as `unpinned:` lines. REFERENCE.md went from 2,227 to
    2,262. The MCP results and views did not change.
  - kern-8b measured 12,834 on the repository's file; this script runs on a temporary copy.
  - The effect: K-view +46 real tokens per run (REFERENCE), K-full about +7,800 (the CLI report).
  - The superseded f4f6674 table is kept under "Earlier version", below the current one.

Rerun from the repository root:

```text
KERN_BIN=<kern built from a clean checkout> .venv/bin/python -I experiments/challenge/c2b/predict.py \
    --tokenizer .venv/lib/python3.9/site-packages/anthropic/tokenizer.json
```

Predicted totals (frozen metric, median of C2's three seeds per cell). "Wide" adds a `spec` view of every direct
caller of what the task changes:

| task | TS (0.7×) | K-view, editor | K-view, spec-first | K-view, spec-first, wide | K-full |
|---|---|---|---|---|---|
| T1 | 22,934 (16,054) | 15,695 (0.68×) | 14,595 (0.64×) | 14,595 (0.64×) | 61,988 (2.70×) |
| T2 | 25,618 (17,933) | 18,608 (0.73×) | 17,324 (0.68×) | 26,274 (1.03×) | 66,913 (2.61×) |
| T3 | 26,452 (18,517) | 21,906 (0.83×) | 19,712 (0.75×) | 20,509 (0.78×) | 62,650 (2.37×) |

**Earlier version** (f4f6674, superseded by the table above before any run; kept because a prediction is not
edited silently). It differed only in REFERENCE.md and the CLI check report:

| task | K-view, editor | K-view, spec-first | wide | K-full |
|---|---|---|---|---|
| T1 | 15,649 (0.68×) | 14,550 (0.63×) | 14,550 (0.63×) | 54,213 (2.36×) |
| T2 | 18,562 (0.72×) | 17,278 (0.67×) | 26,228 (1.02×) | 59,138 (2.31×) |
| T3 | 21,860 (0.83×) | 19,666 (0.74×) | 20,463 (0.77×) | 54,875 (2.07×) |

**The prediction, stated so that the run can contradict it:**
- **T1:** K-view passes 0.7×. With spec-first reading it would still pass if TS read 1,264 fewer proxy tokens
  than modelled.
- **T2:** it's on the line. K-view passes only if the agents read with `spec` *and* trust `kern_check` instead of
  reading the 14 dependants. If they read the dependants, it is about 1.0×.
- **T3:** K-view misses, at 0.75–0.83×. It would pass only if TS read about 1,000 proxy tokens more than modelled.
- **So threshold 3 is predicted to fail** (it needs all three tasks), but by less than in C2: about 0.64–0.83×
  against C2's 1.03–1.33×.
- **K-full is predicted at 2.4–2.7× TS,** almost all of it from the CLI check report.
- **Thresholds 1, 2, 4 and 5 are predicted to pass,** as in C2. That is a weak prediction: every C2 run
  succeeded.

**Where the model is wrong, and which way:**
- **Behaviour held fixed favours K-view.** An agent facing a larger module may read more, and an agent shown
  stubs may ask again.
- **The TS ratio is a guess, and it could go either way.** A real port could be longer or shorter than 3.89×
  s1's port. Its real size replaces the guess before freezing.
- **Partial reads held at their C2 size favours TS** (it's the conservative choice for K-view). Letting them grow
  too changes TS by at most 541 tokens (T3).
- **`kern_list` is a fixed cost of about 3,700 real tokens per K-view run** (2,235 proxy), about 20% of the
  predicted K-view total. It is recorded here as measured; redesigning it is out of scope for this draft.
  - **Any change to `kern mcp` before freezing means rerunning `predict.py`.** A change after freezing is not
    allowed during the run.
- **The calibration (1.65) was fitted on s1's views.** s3's views have the same form, so it should carry over,
  but that is untested.

## Cost, and what Fredrik approves

`predict.py`'s cost lines fit C2's per-token prices: $4 / $8 / $0.20 / $20 per million input / cache-write /
cache-read / output tokens. The fit is exact on all 27 C2 runs. Applied to the predicted runs:

| arm | per run (C2) | 15 runs |
|---|---|---|
| K-view | $0.16–0.27 ($0.13–0.18) | ~$3 |
| TS | $0.21–0.31 ($0.11–0.18) | ~$4 |
| K-full | $0.56–0.77 ($0.14–0.22) | ~$10 |
| **all 45** | | **~$17** (ceiling at the $4 budget cap: $180) |

That is about 1.6M tokens under the frozen metric, or about 7M with cache reads (C2's ratio was 4.1–5.2×). Add the
harness probes before the run, which cost about $0.25 in C2, and replacements for harness faults.

**Fredrik approves:**
1. That C2b runs.
2. The module (s3).
3. N = 5.
4. Whether K-full stays.
5. The thresholds as frozen, including that threshold 3 needs all three tasks.
6. The tool set per arm.

Building the prerequisites below needs no approval: it uses no experiment runs.

## Prerequisites (before freezing; none of them is an agent run of the experiment)

1. **A TypeScript port of `escrow_s3.k`** (`escrow.ts`, `platform.ts` with the 11 state maps, `escrow.test.ts`
   built from the 80 `eg` lines, like the s1 port's tests).
   - It must be behaviour-identical: diff-tested against the interpreter on random calls to all 55 definitions,
     as `experiments/compare/ts/diff.ts` did for s1.
   - Its author is the same model family that designed Kern. C2's caveat carries over.
2. **Reference solutions** for T1–T3 in Kern and TS, agreeing on every hidden case.
3. **Hidden cases:** 600 per task. 300 are targeted at the task:
   - T1: `settle`'s boundaries;
   - T2: amounts around 4,100, with escrow balances to match (C2's worlds stopped at 3,000), through `capture`,
     `purchase`, `capture_order`, `refund`, `resolve_for_seller` and the `checkout_*` and `charge_sub*`
     operations;
   - T3: `move`, `tip` and `charge_late` with `from = to` and amounts of 0 and above.

   The other 300 are random calls over all 55 definitions. C2's world generator sets only balances, so it is
   extended to s3's state maps.
4. **A grader and runner on the Rust toolchain.** C2's `common.py` imports the deleted Python `kern`, and its
   `run.py` expects the pre-2026-11-04 layout.
   - Copy C2's `run.py`, `drive.py`, `grade.py` and `make_hidden.py` into this directory, with every post-run fix
     from C2 kept (below).
   - Run Kern through the `kern` binary (`kern run`, `kern check`, `kern mcp`). The K-full arm's Bash rule
     becomes `Bash(kern check:*)`.
5. **Validation without agents,** as C2's: the references pass 600/600; the untouched module fails part of each
   task; a "`move` only" T3 solution is check-clean but fails the `tip`/`charge_late` cases.
6. **The prediction refreshed** with the TS port's real size and the freeze commit's `kern` (the CLI check
   report, and so K-full's cost, changes with work in progress at the time of writing).
7. **Hashes of every hidden and grading file** (`HIDDEN.sha256.json`), verified before the run.
8. **A quiet toolchain.** Each record holds the binary's hash, the git revision and a dirty flag, and no commit
   to `kern/` lands during the run (C2 waited for the fuzzer for the same reason).

## Harness rules carried over from C2

From C2's protocol and its post-run fixes, all kept:
- **Tools per arm:** `--tools` sets the built-in tools per arm. `--allowedTools` only pre-approves.
- **`--restricted`:** C2 measured ~27.5k tokens of the operator's own setup without it.
- **Permission rules** use `//` plus the real path of the workspace (the single-slash form never matched, which
  voided 18 C2 runs).
- **`KERN_MCP_ROOT`** is set to the workspace. The server refuses modules outside it.
- **Workspaces** are fresh temp directories with only the arm's files: no docs/, CLAUDE.md, AGENTS.md, .why,
  build/ or git history.
- **The validity rule, as narrowed and accepted by Fredrik on 2026-11-04:**
  - A tool outside the arm, or a path outside the workspace, makes a run invalid, refused calls included.
  - A chained Bash command that runs is listed (`chained_bash_executed`) and doesn't invalidate the run.
- **`verify()`** matches absolute paths only, and a path needs a name after its first slash.
- **The first-check-clean marker is positive:** every definition's status line is green for Kern, `ℹ fail 0`
  for node.
- **Check iterations** count only checks that ran.
- **Runs that hit `--max-turns 40` or the $4 budget** are graded as they stand, and the hit is reported.
- **Replacements:** a run voided by a harness fault is replaced and counted, and kept in `runs/void/`. A run is
  never replaced for agent behaviour.
- **Every run is reported individually,** not only as medians. With N = 5, a difference of one run is not
  interpreted.

## Not in C2b's scope

- **Cutting REFERENCE.md** (S1 (c)): C2 froze it. C2b uses it as it is at the freeze commit
  (2,262 proxy tokens at df332d5).
- **Redesigning `kern_list` or the CLI check report.** Both are measured above. A change before freezing means
  rerunning `predict.py`.
- **A different model or a different domain.** The domain is still the ledger, which suits Kern, as in C2.

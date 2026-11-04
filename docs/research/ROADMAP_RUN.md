# Roadmap run (started 2026-11-03, overnight)

Fredrik asked for work on each roadmap item: example applications to test and evaluate
continuously, reviews and second opinions from session kern-90, and staying true to
FUNDAMENTALS.md. Each step's pass conditions are written here **before** its code. Results
are appended under each step, failures included.

Git is blocked (the Xcode licence prompt). Every step is checkpointed in `.checkpoints/` (see
its LOG.md) so it can be committed in order later.

## Summary for Fredrik (morning of 2026-11-04)

**Done:** eight steps, each pre-registered, tested and checkpointed (`.checkpoints/00`–`08`), and
reviewed by kern-90 at every checkpoint. **Not committed: git is blocked** by the Xcode licence
prompt. Run `sudo xcodebuild -license accept`, then commit the checkpoints in order
(`.checkpoints/LOG.md` has the messages).

| step | roadmap | result |
|---|---|---|
| S1 the ratchet sees refusals | C1/C3 fix | C1 re-run: Kern 24/24 (B10 now *visible as a spec change*, not refuted). `check --against REV` was untested (git); 2026-11-04: tested, one bug fixed (S1 result 5) |
| S2 quick + cached check | C6 fix, start of roadmap 2 | Results identical. `--quick` meets C6's thresholds; **the full cold check still fails** them (11.8 s per definition, 92.7 s) |
| S3 MCP server | roadmap 2 | 10 tools, structural edits re-check callers; a real `claude -p` client works |
| S4 loops | roadmap 1, **proposal** | batch.k clean. Found: a spec can't *state* a frame, there's no sum, and events in loops are flagged. kern-90's 7 wrong programs now carry the right tiers, identical across test orders (one frozen condition "failed as written", see p7) |
| S5 assumed handlers | roadmap 1, **proposal** | raffle.k clean with ⊙; satisfiability and scope checks; the host records draws and replays them. My claim about draw_winner needing the assumption was **wrong** (recorded) |
| S6 C2 prepared | roadmap 3 | tasks, arms, grader validated without agents, hidden cases hashed, isolation hole closed. **Not run:** needs your approval (27 runs) and your thresholds |
| S7 proven checks removed | roadmap 4 (small) | ~18% per host call; startup audit, `--audit-rate`, `--paranoid` |

**Decisions waiting for you:**
- the loop syntax and the handler syntax (both proposals, with token tables);
- whether to run C2, and its thresholds;
- the S7 default (skip proven checks, always audit at startup).

**Fredrik's decisions (2026-11-04), all as recommended:**
1. **Loops:** the spelling is accepted. Loops stay a *proposal* until the soundness fuzzer (FZ, below)
   has run clean.
2. **Assumed handlers:** accepted (the coarse ⊙ included). The fuzzer covers them too.
3. **C2:** run it, with CHALLENGE.md's proposed thresholds frozen as they stand. The run waits until
   the fuzzer is done, so the tools don't change under it.
4. **S7 default reversed:** the per-call invariant check is **on** by default; skipping proven
   checks is opt-in (`--skip-proven`, with `--audit-rate`). `--paranoid` is gone (it is the
   default). Revisit once the fuzzer has run clean.

**C2, run 2026-11-04** (details in `experiments/challenge/c2/PROTOCOL.md`, "C2 results"):
- **Runs:** 27/27 valid, every one 600/600, no guarantee broken in any arm.
- **Thresholds:** success, breaks, first-check and iterations pass. **Tokens fail:** K-view
  used 1.03–1.33× TS, because of the fixed language tax on a small module.
- **T3 didn't discriminate:** the task text names the trap.
- **Harness:** 18 first-pass K-full/TS runs were void for harness faults, replaced and
  documented. One frozen rule (chained Bash) was narrowed; your call.

**kern-90's findings on the whole run** (an independent session of the same model, so not a
human review):
1. **S7's rationale overstated it.** The skipped check can't fail *if the prover, interpreter
   and host are correct*, and tonight showed the prover wasn't, until fixed (p3 rendered ✔ on a
   false `ensure` at checkpoint 03). `--audit-rate` is the guard against that, so it's not
   optional decoration. The "B ends at −4" result now applies only to edits made by another
   process *while* the host runs; the startup audit catches edits between runs.
2. **The evidence that loops are sound is thin:** batch.k (5 definitions) plus 7 hand-written
   probes, which found 4 tier bugs and 1 order dependence.
   - Recommendation: a fuzzer of random bounded-loop programs with random, often wrong,
     invariants and ensures, asserting that nothing is ✔ when the interpreter finds a violation.
   - Do this before loops stop being a proposal.
3. **The same for assumed handlers:** 13 conditions plus 6 probes. The satisfiability fallback
   ("not checked") hasn't been exercised by any example that needs it.
4. **Independence:** every review tonight was between two sessions of the same model. The new
   surface (loops, handlers, MCP, host policy) hasn't been used by any other agent. C2 would be
   the first real test of S3.
5. **Process:** git was blocked overnight (2026-11-04: licence accepted; checkpoints 00–08 committed in
   order, `8f6542c`…`b43783a`); S3 ran before S4/S5, against the agreed
   order; the S4/S5 syntax is a proposal; the S7 default is your call; C2 is ready but not run
   (27 runs, $4 cap each).
6. **On the record:** every pre-registered failure was reported as such (the C6 full check, p7
   "nothing ✔", the draw_winner claim).

**Corrections made overnight:**
- In the docs step I told you CLAUDE.md and README linked the new docs. They didn't:
  `/usr/bin/python3` is gated like git, and the edit scripts silently did nothing. Now fixed
  and verified.
- A token measurement written before measuring (20 vs 22) was corrected to the measured 26 vs 26.

Order (sent to kern-90 for review):
- **S1:** the ratchet covers `need`s.
- **S2:** incremental and quick `check`.
- **S3:** an MCP server.
- **S4:** loops.
- **S5:** assumed handlers.
- **S6:** C2 prep.

## S1: the ratchet sees refusals, not only guarantees

**Why:** C1's B10 was missed by Kern, TypeScript and Go. Dropping `part <= amount` from
refund's `need` left a self-consistent spec, and the ratchet compares only guarantees. C3
predicts the same route for "dodging" `need`s.

**Change:** the baseline also records each definition's spec `need`s and its inferred
body-level refusals (`body need …`). `check` reports, and the page shows, "Spec changed since
baseline" when the change takes away something a caller could rely on:
- a guarantee or need was removed;
- a need was added or changed (the call now refuses where it didn't).

Strengthening, such as a new `ensure`, isn't flagged. `check --against REV` reads the baseline
from git (`git show REV:FILE.strength`), not from a file the agent can rewrite.

**Pass conditions (frozen before the code):**
1. **The C1 grader, re-run unchanged with the new baseline, finds every spec-located bug,
   B10 included.** Kern catches 24/24 expressible live bugs (it was 23/24). A dated addendum
   goes in CHALLENGE.md; thresholds aren't touched.
2. **Every C3 route a test can produce is flagged:**
   - remove a `need`;
   - add a dodging `need` in a definition with no `ok otherwise` caller and no covering `eg`;
   - remove `ok otherwise`;
   - rewrite an `ensure`.
3. **No false alarms:** an unchanged module and a strengthened one (an `ensure` added) aren't
   flagged.
4. **Floors:** `check` output for ledger v3–v7 and ledger_large is unchanged when there's no
   baseline file, which is how those runs work. A baseline in the old format (no `need`
   lines) doesn't flag every need as added.
5. **The tamper routes are closed once git works.** `--against REV` ignores a rewritten
   `.strength` file. That's tested whenever `git` runs; while git is blocked the test skips
   and says so.

**kern-90's review, adopted:**
- `--against` gets a fixture test now, and the git path is marked untested.
- C3 gets a dated note saying it will run against post-S1 code.
- S3 before S4/S5 departs from the agreed roadmap order (step 1 before step 2). The
  justification: C2 and roadmap 3 need the view tooling, not loops. Recorded in
  .checkpoints/LOG.md for Fredrik.

### S1 results

1. **pass:** the C1 grader (unchanged, frozen at `d3db476`) re-run after S1: Kern catches
   **24/24** expressible live bugs (it was 23/24). B10 is now caught by the ratchet alone, but
   as a *visible spec change* for a human to judge, not a refutation. Any `need` edit
   triggers this, bug or not (kern-90). P1
   is still 13/13; TS and Go are still 13/24 each. Raw data: `experiments/challenge/c1/results_after_S1.json`.
2. **pass:** tests `test_s1_*` cover:
   - a dropped need (B10's exact edit);
   - a dodging need in a module with no `ok otherwise` and with the covering `eg` removed
     (flagged as a "new refusal");
   - `ok otherwise` removed;
   - an `ensure` rewritten.
3. **pass:** an unchanged module and a strengthened `tip` (a buyer `ensure` added) aren't
   flagged.
4. **pass:** `check` output for v3–v7 and ledger_large is byte-identical, and an old-format
   baseline doesn't flag needs.
5. **untested:** git is blocked, so `test_s1_against_a_committed_baseline_ignores_the_file`
   skips and says why. The code path is `__main__.py`, `--against`.
   - **2026-11-04, once git worked: pass after one fix.** From the repo root, a dropped
     `need from != to` hidden by a rewritten `.strength` file is flagged against `HEAD`
     ("SPEC CHANGED SINCE BASELINE", exit 1). From a subdirectory, `--against` failed
     (exit 2, "no baseline"): it passed `git show` a path relative to the working directory,
     which git reads from the repo root. The fix is `REV:./path`. The test now runs the real
     CLI in a scratch repo from both places; it fails without the fix and passes with it.

**Also found while doing S1:** `/usr/bin/python3` is behind the same Xcode licence as git. Edit
scripts run with it since ~22:00 had printed the licence message and exited without changing
anything:
- the CLAUDE.md and README links I reported as done in the docs step were never written;
- the first try at the S1 code didn't land either.

Both are now redone with `.venv/bin/python`, and each edit is checked after it runs.

## S2: `check` in the inner loop (incremental, quick, faster matching)

**Why:** C6 failed both points (16.5 s per definition at d=6/k=4/e=6; 110 s at n=200).
- Mutation testing is ~95% of the time, and `check` re-proves everything on every run.
- kern-90 profiled it: Z3 is 3.3 of 171 s. 92% goes to building terms in
  `conditional_emit_goal`, where prefix sums are rebuilt from scratch, O(n²).

**Change:**
1. **Incremental prefix sums** in `conditional_emit_goal`. kern-90 measured this as giving
   identical results on three C6 configurations.
2. **A cache** of each definition's results. The key is the SHA-256 of:
   - the tool's own code (`kern/*.py`), so a prover fix invalidates everything;
   - the module's declarations (state, invariants, channels);
   - the definition's content hash;
   - the content hashes, bodies included, of every definition it reaches transitively,
     because callers depend on what's inferred from callee bodies (`lifted`, `writes`,
     `fails`, `uses`);
   - whether mutation testing ran.

   It's conservative: a callee body change re-checks its callers. It's stored in
   `.kern-cache/`, and trust propagation and the ratchet are recomputed every run.
3. **`check --quick`:** proofs, examples and random tests, but no mutation testing. Spec
   strength reads "not measured (--quick)" unless the cache holds a full result for the
   *same* key.

**Pass conditions (frozen before the code):**
1. **Identical results.** With the speed change and a cold cache, every clean flag, guarantee
   status, counterexample `eg` and mutation score on escrow s1/s2/s3, ledger v3–v7,
   ledger_large and the C6 configurations P1 and P2 is the same as before S2 (SHA-256 over
   them).
2. **Warm cache equals cold cache.** The second run's terminal output is byte-identical to
   the first's.
3. **Incremental:** in a 30-definition generated module, changing one leaf's `eg` re-checks
   only the leaf and its transitive callers, and an unchanged second run takes ≤ 10% of the
   cold run.
4. **Cache safety:**
   - after a change to a callee's body, the caller's result is not served from the cache;
   - a change to a tool file invalidates every entry;
   - a corrupt cache file is ignored, never trusted.
5. **`--quick`** never prints a strength it didn't measure for the current key.
6. **C6 addendum** (thresholds unchanged): P1 and P2 re-measured for the full cold `check`
   and for `--quick`, reported as measured. A miss stays a miss.

## S3: an MCP server, so an agent works through views (roadmap 2)

**Why:** the central claim is that an agent can work from views without reading bodies. Today
the views exist only as CLI output, and C2's K-view arm needs a real interface: views, why,
check, and structural edits that re-check what they affect (F2: callers were verified
against the old spec).

**Change:** `python -m kern.mcp`, an MCP server on stdio (JSON-RPC 2.0, no dependencies), with
these tools:
- `kern_list`, `kern_view`, `kern_why`, `kern_check`, `kern_run`;
- `kern_add`, `kern_replace`, `kern_remove` (structural edits on whole definitions).

On an edit, the whole new module is parsed and well-formedness-checked first. If it's
ill-formed, the file is unchanged. Otherwise the file is written, the edited definition and
all its transitive callers are re-checked (quick, with the cache), and the result is `ok`
only if every one of them is clean; otherwise it names what broke. `kern_remove` refuses
while the name is referenced. There's no "read the whole file" tool.

**Pass conditions (frozen before the code):**
1. **A scripted stdio client completes** `initialize`, `tools/list` and every tool, and every
   response is valid JSON-RPC.
2. **`kern_view` output is byte-identical** to `python -m kern view` for every definition and
   view of escrow_s1.
3. **A replacement that breaks a caller's guarantee returns `ok: false` and names that
   caller.** An equivalent replacement returns `ok: true`, and the file's bytes outside the
   replaced definition are unchanged.
4. **An ill-formed edit leaves the file byte-identical,** and the tool reports the error.
5. **The re-check after an edit re-proves only the edited definition and its callers**
   (everything else comes from the cache).
6. **Every tool is documented in GUIDE.md** (test enforced).
7. **A scripted end-to-end task done through the server only:** add a new definition to a
   copy of escrow_s1 using view, add and check, ending clean.

**Added after kern-90's review** (the code was drafted, but nothing had been run against these
conditions yet):
- **Failing edits:** write the file and return `ok: false`. A coordinated change, callee first
  and then callers, is red in between, so rolling back would make multi-definition changes
  impossible.

New conditions:
8. **`kern_edit(file, ops)`** applies several add/replace/remove operations together and checks
   once. If the result is ill-formed, nothing is written.
9. **Every view shows the status of what it shows.** A spec view of a red callee, inside an
   editor view, is not presented as proven.
10. **Edit results list** the `why` entries the edit made stale, and `remove` lists the entries
    it orphaned.
11. **`kern_callers(name)`** lists the definitions that reach it.
12. **A per-session JSONL log** of tool, arguments and response tokens (`KERN_MCP_LOG`), so C2 can
    separate tokens read through views from total usage.
13. **`kern_check`** defaults to a summary (status per definition, plus the ✗/? lines with
    counterexamples), with a `name` filter for detail.
14. **`expect_hash` on replace and remove:** refused if the definition changed underneath.
15. **A real-client smoke test:** `claude -p` with the server in `--mcp-config` calls
    `kern_list` successfully. One short run, recorded with its usage.

### S2 results

1. **pass:** the digest over every clean flag, guarantee status, counterexample-confirmed flag
   and mutation score for 10 modules is identical before and after: `3cb0e5efa3b63fb8`. The
   modules are escrow s1–s3, ledger v3–v7, ledger_large and a C6 configuration (4,3,4,8).
   - Witness *values* weren't compared; they already vary with solver history.
   - kern-90 separately found identical results on P1, P2 and (3,2,8,3) for the same
     prefix-sum change.
2. **pass:** with the cache cold and then warm, the digest is identical, and the terminal
   output of `check` is byte-identical (escrow_s3, ledger_v5, ledger_v6). The warm run on 10
   modules takes 0.1 s against 37.2 s cold.
3. **pass:** 30 generated definitions. Unchanged second run: 0.019 s, against 13.6 s cold
   (0.14%). One leaf's `eg` changed: 1.35 s, re-checking exactly the leaf and its callers
   (`f4_1`, `f4_2`, `f4_3`).
4. **pass:** `test_s2_*` covers a callee body change (never served from the cache), a tool
   change (everything invalid) and a corrupt cache entry (recomputed).
5. **pass:** `--quick` shows "not measured (--quick)". It shows a cached strength only for an
   identical key, and not after any change.
6. **C6 re-measured** (thresholds unchanged; `experiments/challenge/c6/results_after_S2.json`):

| | before S2 | after S2, full cold `check` | after S2, `--quick` |
|---|---|---|---|
| P1: ≤ 1 s per definition (d=6, k=4, e=6) | 16.46 s | **11.77 s: still fails** | 0.36 s |
| P2: ≤ 60 s (n=200) | 110.0 s | **92.7 s: still fails** | 6.8 s |

   The prefix sums bought ~28% and ~16%. The full check still misses both thresholds; the
   inner-loop modes (`--quick`, and the cache after an edit) meet them. Mutation testing is
   still the cost, and the right place for it is a full check before commit, not every edit.

### S3 results

All 15 conditions pass: `tests/test_mcp.py` covers 1–5 and 7–14, `tests/test_docs.py` covers 6,
and 15 was run by hand:
- **1:** a scripted stdio client covers `initialize`, the `initialized` notification,
  `tools/list`, every tool, an unknown method (-32601) and a tool error (`isError`, not a
  crash).
- **2:** `kern_view` output is byte-identical to `kern view` for 3 definitions × 3 views.
- **3:** breaking `transfer`'s credit returns `ok: false` with `transfer` *and* its callers in
  `broken`. An equivalent reordering returns `ok: true`, and the text after `transfer` is
  untouched.
- **4:** two ill-formed replacements leave the file byte-identical.
- **5:** an equivalent body change to `split` re-checks only `split`.
- **7:** `refund_all` was added through `kern_view` / `kern_add` / `kern_check` only, and is
  clean.
- **8:** a two-op edit with one ill-formed op writes nothing; the valid pair applies together.
- **9:** with `transfer` broken, `reserve`'s editor view shows transfer's section as `proof N
  open`, not `proof ok`.
- **10:** a body change makes payout's `why` stale and is reported; removing payout reports
  the orphaned entry. An `eg`-only change doesn't stale it, as designed.
- **11, 12, 13, 14:** callers, the token log, the summary/detail `check`, and `expect_hash`.
- **15:** `claude -p --mcp-config … --strict-mcp-config --allowedTools mcp__kern__kern_list`
  answered "10 definitions. capture: All guarantees proven." That's 3 turns, $0.28, and the
  protocol worked with a real client.

**Design decisions taken with kern-90:**
- A failing edit is written with `ok: false`, so you fix forward (coordinated changes are
  red in between).
- There's no whole-file read tool.
- **For C2:** the K-view arm must run with only `mcp__kern__*` tools allowed, and the
  transcript must be checked for file reads (C2 prep, S6).

## S4: bounded loops with invariants (roadmap 1). This is a proposal for Fredrik.

**Why:** batch work (sweep every expired hold, advance every subscription) can't be expressed.
C5's feature 6 (revoke every credential of a holder) needs a loop too.

**Proposed syntax.** Token costs were measured for the alternatives; all are within 2 tokens,
so readability decided:

| form | chosen | tokens | alternatives (tokens) |
|---|---|---|---|
| loop | `for k from LO to HI` (HI exclusive, both evaluated once, `k` immutable) | 8 | `for k in lo hi` 7, `each k from lo to hi` 8, `for k lo hi` 6 |
| loop invariant (the loop's first line) | `invariant COND` | 16 | `keep …` 16, `holds …` 16, `so far …` 17 |
| quantified clause | `ensure E [when C] for j from A to B` (for every j in [A, B)) | 25 | `ensure all j from lo to hi: …` 26, `… each j in lo hi: …` 25 |
| per-iteration event (optional) | `emit Ev j for j from A to B` | 12 | `emit each Ev j …` 13 |

- **The loop invariant reads like an `ensure` about "the state so far".** `x'` is the state
  before iteration k, unprimed is the call's before-state, and k is the next index.
- **`from` and `to` are common parameter names,** so loop and quantifier bounds must be atoms
  (a name, a number, or parenthesised).

**Prover:**
- The invariant must hold on entry, at k = LO.
- One arbitrary iteration with LO ≤ k < HI keeps it, with the stores the loop writes havocked.
- After the loop, it holds at k = max(LO, HI).
- **Inferred frame:** a state map written only at the loop variable keeps every key outside
  [LO, k), so nobody writes that frame by hand (F4).
- **Module invariants** are assumed for every key in the before-state and checked over
  [LO, HI) after the loop, so they stay ✔ (kern-90's risk).
- **Events emitted inside a loop** make that channel opaque. It's unconstrained and flagged,
  and never ✔ (kern-90). The per-iteration form is optional for S4.
- **Z3 unknown or timeout** stays `?` (or ◐), never ✔.

**Example app:** `examples/batch.k`:
- `sweep_expired`: expired holds go to an unclaimed-funds account;
- `advance_subs`: due subscriptions are rolled forward;
- `installments`: a fixed instalment, n times.

A known gap to record: anything needing a *sum* ("the pool gains the sum of the expired holds")
can't be stated, because there's no sum operator; only conservation between two accounts can.

**Pass conditions (frozen before the code):**
1. `examples/batch.k` is clean (every definition ✔), module invariants included, and every
   page claim comes from a template.
2. **Adversarial tests,** each refuted with a counterexample that replays:
   - an invariant not kept by an iteration;
   - an invariant false on entry;
   - an off-by-one bound;
   - a quantified `ensure` false at one key.
3. **A loop-written map's frame is usable by callers:** a caller proves a key outside [LO, HI)
   unchanged.
4. **Events emitted in a loop are never ✔ for that channel,** and the page says why.
5. **Floors:** the S2 digest over escrow s1–s3, ledger v3–v7, ledger_large and C6 (4,3,4,8) is
   unchanged, and so are the build hashes.
6. **Docs:** REFERENCE.md documents the forms and stays ≤ 2,500 tokens; GUIDE.md covers loops.
7. The syntax is recorded as a **proposal** Fredrik can reject (the token table above).

**Added after kern-90's review of S4** (before any S4 code):
- **(a) The "for all keys" invariant assumption is justified only if every definition that
  writes the store proves the invariant.** So a proof that uses it records
  `invariant on STORE` as relied on, like a callee. Trust propagation downgrades it to `!` when
  some writer of that store doesn't prove the invariant ✔.
- **(b) One rule, everywhere:** unprimed = the state before the call; primed = the state at the
  point the clause is about (after the call for `ensure`/`keep`, the start of the iteration for
  a loop invariant).
  - REFERENCE.md says so, and the page says "at the start of each iteration".
  - Measured: sweep's conservation invariant is 26 tokens in this form and also 26 with an
    `old` marker (`old ledger.balance escrow …`), so the choice is about reading, not tokens.
    (A first draft of this line said 20 vs 22, written before measuring; corrected.)
- **(c) Refusals inside a loop:**
  - they're proven in the arbitrary iteration (invariant + LO ≤ k < HI + spec needs), not only
    at entry;
  - they're never lifted to callers;
  - a loop that can refuse makes the definition refuse beyond its spec unless the spec says
    `ok otherwise`, and `fails` includes the loop's failures.
  - Refusal arguments computed from loop state are checked by `eg` only.
- **Loop mutants (F8):** LO ± 1, HI ± 1, the key off by one (`k` → `k + 1` in a key), and an
  emptied body. Otherwise strength is blind to the bugs loops introduce.

New conditions:
8. A proof that rests on the quantified invariant assumption shows `!` if some writer of the
   store doesn't prove the invariant (a test with a deliberately unproven writer).
9. **Loop mutants exist,** and batch.k's strength is reported with them, including what they
   miss.
10. **Timing for batch.k** (quantified goals are where check time can jump): `--quick` takes
    ≤ 1 s per definition, and the full `check` time is reported. Every Z3 goal keeps its 5 s
    timeout, and a timeout or unknown is `?`, never ✔.

### S4 results

1. **pass:** `examples/batch.k` (5 definitions) is clean: every guarantee ✔, including the
   module invariants kept across loops and 24 loop-invariant goals.
2. **pass,** tests `test_s4_*`:
   - an invariant an iteration doesn't keep, and one false on entry, are refuted;
   - an off-by-one bound (`0 to (n + 1)`) is refuted with a replayed `eg`;
   - a wrong condition (`<=` → `<`) is refuted at the loop invariant, with the witness key in
     the `eg` line (`sub_due.797:…`).
   - **The replay needed a fix.** A quantified goal's failing key is a Z3 witness, not a
     parameter, so counterexamples now carry every map key the model assigns (Store chains
     and as-array tables). Before that fix, "drop the whole loop" survived as unconfirmed.
3. **pass:** `sweep_elsewhere` proves `hold.get' other = hold.get other` from sweep's inferred
   frame `hold.[lo,hi)`. Without `need other < lo or other >= hi` it isn't provable, which
   is correct.
4. **pass:** events in a loop make the channel opaque. An `emit` clause over them is
   "unknown: it emits events inside a loop", and the terminal and page add a note even when
   there's no clause.
5. **pass:** the S2 digest is still `3cb0e5efa3b63fb8` over all 10 modules, and the build hashes
   are unchanged.
6. **pass:** REFERENCE.md (2,029 tokens) and GUIDE.md ("Loops", with a `kern clean` example) are
   updated, and test_docs passes.
7. **Proposal:** the syntax is recorded above with its token alternatives, for Fredrik to
   accept or reject.
8. **pass:** with an unproven writer of `hold` added (`spoil`), every proof in `sweep_expired`
   becomes `!`, blocked by "invariant on hold".
9. **Loop mutants exist** (bounds ± 1, `k` → `k + 1`, an emptied body). batch.k's strength with
   them:

| definition | guarantees | what survives, and why |
|---|---|---|
| sweep_expired | 11/18 | **`lo - 1` and `hi + 1` survive:** the frame is inferred, never stated, and a bounded quantifier can't say "every key outside [lo, hi)". **Dropping the transfer, or swapping escrow and pool, is caught only by examples:** the spec can't say "the pool gains the sum of the swept holds" (there's no sum). `hold.get k > 0` → `>= 0` survives because sweep can't say `ok otherwise` (its Insufficient condition needs a sum) |
| advance_subs | 14/16 | `lo - 1` and `hi + 1` (the same frame gap) |
| installments | 10/10 | — |
| sweep_elsewhere | 3/7 | a weak wrapper spec (it promises only the key outside the range) |

10. **pass:** full `check` of batch.k takes 8.6 s (1.7 s per definition); `--quick` takes 1.74 s
    (0.35 s per definition, within ≤ 1 s). No goal timed out.

**Findings, ranked:**
1. **A spec can't state a frame.** "Every key outside [lo, hi) is unchanged" needs an unbounded
   quantifier (`for j outside lo to hi`, or `except`). The prover infers the frame and
   callers use it, but the definition itself is blind to violating it. That's the same
   inferred-not-stated gap as writes in F3.
2. **No sum.** Conservation is expressible only pairwise. A `sum j from A to B of E` form, with
   loop-invariant support, would close the strongest survivors.
3. **Events in loops** need the per-iteration form (`emit Ev j for j from A to B`). It's not
   built; the channel is flagged instead.

## S5: handlers declared in Kern, with assumed specs (roadmap 1). This is a proposal.

**Why:** an observation *inside* a call (randomness, an environment value, a file read) has no
handler today. HOST_PLAN deliberately left it out. Declaring it in Kern, with a spec the prover
*assumes*, keeps the closed world (F7) and keeps the page honest (F5): a proof that rests on an
assumption must say so.

**Proposed syntax:**

```text
handler rand.below n:Int -> Int
  need n > 0 else BadBound
  ensure result >= 0 and result < n
```

A column-0 `handler NAME params -> Int` with spec lines and no body.

**Semantics:**
- **Prover:** every call gives a fresh value satisfying the assumed `ensure`s. Unlike a function,
  two calls may differ.
- **Trust:** a proof that uses the value records "relies on assumed `rand.below`". The page lists
  the assumption and marks those guarantees, distinct from ✔.
- **Interpreter:** a provider supplies the value (seeded in tests, the host in production), and
  the assumed `ensure` is *checked at runtime*. A violation is a host fault, never silent (tier ○).
- **Host:** every observation is recorded in the call log, and replay feeds the recorded values
  back (F7).

**Example app:** `examples/raffle.k`:
- `draw_winner` pays one of three accounts from `rand.below 3`. Its guarantee "the pot pays
  exactly `amount`" holds only because the assumed spec says the result is below 3.
- `pick_order` picks an order id in [lo, hi).

**Pass conditions (frozen before the code):**
1. **raffle.k is clean,** and every guarantee that rests on `rand.below` is shown as relying on
   the assumption (terminal and page), never as a plain ✔.
2. **A guarantee that needs more than the assumed spec isn't provable:** with
   `ensure result >= 0` only, draw_winner's pot guarantee fails, and `r1 = r2` for two calls
   isn't provable.
3. **A provider that breaks the assumed spec at runtime is caught** (a random test or a host
   call reports a fault, not ok).
4. **Host replay is deterministic:** 200 host calls with a seeded provider replay exactly from
   the log.
5. **The manifest lists the assumed handler** as an effect the module can reach.
6. **Floors:** the S2 digest is unchanged.
7. **Docs:** REFERENCE (within budget) and GUIDE.

## S4 fix-up: tiers that misled (kern-90's audit of checkpoint 03; done before finishing S5)

kern-90 wrote 7 deliberately wrong loop programs (now `tests/probes/p1..p7.k`). None was clean,
so the gate held. But four showed tiers that mislead a human, which breaks design rule 6:
- **p3:** an `ensure` showed ✔ while the loop invariant it follows from was ✗.
- **p4:** a definition that breaks the invariant it relies on was masked as `!` by relying on
  itself.
- **p7:** a false loop invariant made everything ∅ "vacuous", with the wrong explanation.
- **p5 and p2:** real bugs showed as "unconfirmed", because the witness came from unreachable
  havocked state.

**Pass conditions (frozen before the fix).** Each probe is non-clean *and* carries the tier
below. "✗" means refuted, with a counterexample that replays in the interpreter.

| probe | the bug | required |
|---|---|---|
| p1 | the ensure promises more than the loop does | that ensure ✗ |
| p2 | a write at `k + 1` reaches `other = hi` | that ensure ✗ |
| p3 | the invariant says +2, the body adds 1 | loop goal "kept by every iteration" ✗, **and** the ensure is `!` (relies on the refuted loop invariant), never ✔ |
| p4 | `hold.set k (hold.get k - 1)` breaks `hold >= 0` | a module-invariant goal ✗, not masked by its own reliance |
| p5 | `ok otherwise` with no balance need | `ok otherwise` ✗ |
| p6 | an empty range (lo ≥ hi) | that ensure ✗ |
| p7 | `invariant 1 = 0` | the loop's entry goal ✗; nothing ✔; any ∅ explanation names the loop |

Also the S2 digest is unchanged, and batch.k stays clean.

### S4 fix-up results

| probe | required | result |
|---|---|---|
| p1 | that ensure ✗ | **pass** (replayed) |
| p2 | that ensure ✗ | **pass**. It was flaky in the full suite (an in-process witness that didn't replay), so for quantified goals the counterexample search now tries up to 4 more models, each with different parameters, until one replays |
| p3 | loop "kept" ✗, ensure `!` | **pass**. The ensure and the module-invariant goals are `!` "relies on loop invariant … (kept by every iteration)" |
| p4 | module invariant ✗, not masked | **pass**: `eg hold.130:848 … drain 128 130 -> ok \| hold.128:-1 hold.129:-1`, replayed with loop checks on |
| p5 | `ok otherwise` ✗ | **pass**: `eg A:0@u1 B:0@u1 \| pay_n A B 1 -> Insufficient 10`. For a refusal the search now *minimises* balances |
| p6 | that ensure ✗ | **pass** (`touch_first 282 281`: an empty range) |
| p7 | entry ✗; nothing ✔; ∅ names the loop | **entry ✗: pass. ∅ names the loop: pass. Nothing ✔: fails as written.** One ✔ remains, "module invariant `hold >= 0` holds when the loop starts". It's a true claim that doesn't depend on the false invariant, since entry goals assume no invariant. Every "kept" goal is now `!`, because induction assumes all of a loop's invariants |

**What changed:**
1. **Trust:** a goal that follows from a loop (any non-loop goal, and every "kept" goal) relies on
   all of that loop's goals, and is `!` when one isn't proven.
2. **No self-masking:** a definition's own invariant-preservation goals don't rely on the store
   invariant through *itself*.
3. **Vacuity doesn't swallow loop goals,** which have premises of their own; the ∅ explanation
   names the loop.
4. **Replaying loop counterexamples:** the interpreter can check loop and module invariants at
   each iteration head (used only to confirm counterexamples).
5. **A loop that runs no iterations changes nothing,** which stops witnesses from coming out of
   unreachable havoc.

**Floors:** the S2 digest is still `3cb0e5efa3b63fb8`, batch.k is clean, and the suite runs
green twice in a row (288 passed, 1 skipped, 2 xfailed).

**S5, added after kern-90's review.** Part of the S5 code existed already: parsing, the
provider, the ⊙ tier. These conditions were frozen before it was finished or measured.

8. **A handler `need` is a precondition the caller must prove at each call** (its own goal,
   ✗ or ? when it can fail). At runtime a violation is an error, not a refusal, and is not
   among the inferred failures.
9. **An assumed spec may read only its arguments and `result`.** Ledger or state reads are
   rejected (no unproven frame or guarantee can hide in an assumption).
10. **Satisfiability:** ∀ args. needs → ∃ result. ensures is checked. If it's unsatisfiable,
    callers relying on it are not ✔ (`!` "its spec can't be met"). If Z3 can't decide, the
    page says "not checked".
11. **The ratchet sees assumptions:** changing a handler's assumed spec shows SPEC CHANGED in
    every definition that relies on it.
12. **The host** logs every draw in call order, replays from the log, refuses at startup a
    handler it can't provide, and the manifest lists `rand`.
13. **The page and terminal list the module's assumptions at the top,** each with its
    satisfiability status.

### S4 fix-up, part 2: tiers must not depend on test order (kern-90's audit of checkpoint 04)

The p2 tier flipped between ✗ and `!` with earlier solver calls in the same process: it failed
under `pytest -k probe`, which imports test_host.py, and passed alone.

The cause: p2 writes at `k + 1`, which the loop analysis treated as "any key". The whole map was
havocked, so Z3 offered witnesses at far-away keys that a real run can't reach (`other = 998`
with lo=0, hi=1). Which witness came first depended on solver history.

**Changes:**
1. **A write at the loop key plus a constant** (`k + c`) gets a shifted frame inside the loop
   (keys outside [lo+c, k+c) unchanged), so it's precise.
2. **A small-world attempt first** for quantified goals (integer parameters in 0..5), then up to
   8 alternative models.

**Pass condition (frozen before the change):** the tier of every probe goal (p1–p7) is identical
under three orders: the whole suite in default order; the test files in reverse order; and the
probe file alone.

**Result: pass.** The tiers of all probe goals are identical across four orders:
- the whole suite;
- the test files reversed;
- the probe file alone;
- `pytest -k probe` over all files, which failed before the change.

The suite is green in both full orders (299 passed, 1 skipped, 2 xfailed), and the S2 digest is
unchanged.

**p7's remaining ✔ (dated note, 2026-11-04).** kern-90 agrees it's honest: "module invariant
holds when the loop starts" is true and independent of the false loop invariant. The frozen
condition "nothing ✔" stays as written and is reported as **failed as written**. It was
over-strict, but it isn't edited (rule 1).

### S5 results

| # | condition | result |
|---|---|---|
| 1 | raffle.k is clean; guarantees resting on `rand.below` are marked | **pass.** Status "Proven, assuming rand.below"; ⊙ on each guarantee; `ASSUMES …` at the top of the terminal output and an "Assumptions" section on the page |
| 2 | a guarantee needing more than the assumed spec isn't provable | **pass for the mechanism, wrong about the example.** pick_order's `picked < hi` is not provable with `ensure result >= 0` only, and two draws may differ (`a - b = 0` isn't provable). But I had claimed draw_winner's pot guarantee needs `result < 3`, and it doesn't: its `else` branch pays for any result ≥ 2. That claim was wrong, and the test now says so |
| 3 | a provider breaking the assumed spec is caught | **pass:** a provider returning 7 for `rand.below 3` raises "broke its assumed spec", and the world is unchanged (atomic) |
| 4 | host replay is deterministic | **pass:** 400 host calls with a seeded provider; every draw is in `log.observed` in call order, and `replay()` is clean |
| 5 | the manifest lists the handler | **pass:** `rand: assumed handler rand.below: the OS random source …` |
| 6 | floors | **pass:** S2 digest `3cb0e5efa3b63fb8` |
| 7 | docs | **pass:** REFERENCE.md is 2,126 tokens; GUIDE.md covers the host part |
| 8 | handler `need` = call-site precondition | **pass:** `rand.below (hi - lo - 5)` gives ✗ on the call-site goal, with a replaying `eg` ("error: need of rand.below not met"); `BadBound` isn't among pick_order's failures |
| 9 | assumed specs read only args and `result` | **pass:** `ledger.balance n` and `hold.get n` in a handler spec are rejected |
| 10 | satisfiability | **pass:** `result >= n and result < n` → "unsatisfiable", and every caller's guarantee is `!` "its spec can't be met" |
| 11 | the ratchet sees assumptions | **pass:** changing `< n` to `<= n` gives SPEC CHANGED in draw_winner ("need assumes rand.below: …") |
| 12 | host: log, replay, refuse unprovided | **pass:** a host with no provider for `rand.below` refuses to start (ConfigError) |
| 13 | assumptions at the top | **pass** (see 1) |

**Coarseness to know about:** ⊙ marks *every* proven guarantee of a definition that reaches an
assumed handler, not only those whose proof needed it. That's conservative; precise marking
would need a re-proof without the assumption.

**S5 follow-ups (kern-90's audit of checkpoint 05, which passed):**
- **The vacuity explanation** "no input gets past a loop" had leaked into definitions without a
  loop, because handler preconditions share the extra-goal list. It now appears only when there
  is a loop goal.
- **An unsatisfiable assumption says for which arguments,** e.g. "spec unsatisfiable for n = 5".
- **Counterexamples that depend on a draw** now replay: each path records its handler results
  in call order, and replay feeds the model's values back. kern-90's h4 and h6 went from `?` to
  ✗.
- **The coarse ⊙ is accepted for now** (kern-90). A later refinement: re-prove each ⊙ goal
  without the assumed `ensure`, and if it still proves, show plain ✔.

## S6: C2 prepared, not run

`experiments/challenge/c2/`:
- `tasks.md`, `run.py` (arm isolation, dry-run by default), `grade.py`, `common.py`,
  `harness.ts`, `make_hidden.py`;
- `hidden/` (reference solutions and 3 × 600 acceptance cases);
- `HIDDEN.sha256.json` (10 files) and `PROTOCOL.md`.

Everything kern-90 asked for:
- **T3 says what callers must do:** every other operation keeps its behaviour, so `tip A A 0`
  must still succeed. That makes T3's trap measurable.
- **Tool isolation per arm,** checked from the transcript.
- **The token metric** is defined, and all four usage numbers are reported.
- **No one-run differences** are interpreted at N = 3.
- **The property suite** derived from Kern specs is frozen, hashed and outside every workspace.

`grade.py --validate` (no agents):
- the references succeed (600/600 for each task);
- untouched code fails;
- a T3 "move only" variant is caught as breaking tip's `ok otherwise`, silently in Kern (spec
  unchanged) and in TS (no spec).

**Not run:** 27 runs need Fredrik's approval, and Fredrik freezes the final thresholds.

## S7: proven checks removed at runtime (roadmap 4, small)

**Why:** the host checks module invariants after every successful call (tier ○, at the keys the
call wrote). For a definition whose invariant goals are all proven ✔, that check can't fail
unless the host's state was changed outside Kern, so it's pure cost.
**Correction, 2026-11-04 (kern-90):** that holds only *if the prover, interpreter and host are
correct*. Tonight's p3 showed the prover can be wrong, and `--audit-rate` is the guard. Removing it is what
verification is supposed to buy.

**Change:**
- The host skips the runtime invariant check for definitions whose `invariant` goals (and loop
  module-invariant goals) are all ✔.
- It keeps the check for every other definition (`!`, `?`, ◐, ✗ with the gate off). For ⊙,
  the assumed-handler check stays on.
- `--paranoid` keeps every check on, for state edited outside Kern. Replay detects tampering
  anyway.
- The manifest says, per definition, which runtime checks are removed and why.

**Pass conditions (frozen before the code):**
1. `test_sqlite_host_matches_the_interpreter` (600 calls) still passes.
2. The gate-off BURN test still catches its broken invariant at runtime.
3. A measured per-call speedup for `transfer` on escrow_s2 at 5,000 accounts (before and after,
   in memory), reported as measured.
4. The manifest says which checks are removed, and `--paranoid` restores them (tested).

### S7 results

1. **pass:** `test_sqlite_host_matches_the_interpreter` (600 calls) passes.
2. **pass:** BURN (unproven, gate off) still has the check on and still catches its broken
   invariant at runtime.
3. **Measured:** `transfer` on escrow_s2, in memory, 300 calls. Per-call time went from 0.034 /
   0.038 / 0.038 ms to 0.028 / 0.031 / 0.031 ms at 50 / 500 / 5,000 accounts, roughly −18%. That's
   small, because H1 had already made the check key-scoped.
4. **pass:** the manifest shows `runtime invariant check removed (proven)` or `on` per
   definition, and `--paranoid` restores the check (tested).
   - **The trade-off, now tested:** with a balance made negative outside Kern, a non-paranoid
     host lets the next call through (B ends at −4); `--paranoid` refuses it with an error.
     Since the startup audit (S7 additions), that applies only to edits made by another
     process *while* the host runs.
   - Replay detects the tampering either way.

**S7 additions (kern-90's review):**
- **A full invariant audit at host startup, always.** The host refuses to serve a database
  whose invariants don't hold. That closes "tampered between runs" (another process, a
  migration, an older binary), which replay would only catch when someone runs it. Tested.
- **`--audit-rate R`:** also runs the skipped checks on a sampled fraction of calls. That's
  defence in depth against host/interpreter bugs, which the proofs say nothing about. Counts are
  in the manifest; tested at R = 1.
- **The skip applies only where every invariant goal of the definition, as served, is ✔.**
  `!`, `?`, ◐, unknown (e.g. a computed key outside a loop frame) and ⊙ keep the check on.
- **The default** (skip proven checks, audit at startup) is a deliberate choice, recorded here
  for Fredrik. `--paranoid` turns everything back on.
  - **2026-11-04, Fredrik: reversed.** Checks are on by default; `--skip-proven` opts in to the skip
    (and `--audit-rate` samples it). `--paranoid` was removed. Tests renamed to match.

**C2 isolation hole (kern-90's audit of checkpoint 06), closed:** file tools are scoped to the
workspace, the MCP server is rooted at the workspace, a run is invalid if any tool call names
an outside path or chains Bash commands (tested on a synthetic transcript), and each run has a
$4 cap. `HIDDEN.sha256.json` is re-hashed, and PROTOCOL.md updated.

## FZ: soundness fuzzer for loops and assumed handlers (2026-11-04, decided by Fredrik)

**Why:** kern-90's 7 hand-written probes found 4 wrong tiers and 1 order dependence. That's too
high a defect rate to call loops and handlers sound from one example app each. Loops stay a
proposal, and the host keeps every runtime check on, until this runs clean.

**What it is:** `validation/fuzz.py`. It generates programs, runs the normal checker
(`analyse(mutation=False)`), then runs each definition in the interpreter on random small worlds.
The interpreter is the oracle; it shares no code with the prover's Z3 encoding.
- **Programs come from two sources:**
  - random loop programs (bounds, offsets, conditions, writes, invariants and ensures, often
    wrong) and random assumed-handler programs (random assumed specs, draws used in writes and
    loop bounds);
  - every well-formed mutant (`check.mutants`) of the loop and handler templates: batch.k,
    raffle.k, the probes p1–p7. Each is checked in a module holding only it and its callees.
- **A contradiction** is interpreter evidence against a claim on the page:
  - a goal shown ✔ or ⊙ that a run violates: an `ensure`/`keep`/`emit` after an ok run; a
    module invariant after an ok run from a valid world; `ok otherwise` refused with every need
    true; a handler precondition not met at a call;
  - a loop goal: the first iteration head where any loop or module invariant fails. If that is
    the loop's start, a failing invariant's "holds when the loop starts" goal must not be ✔/⊙.
    Otherwise its "kept by every iteration" goal must not be (every invariant held one head
    earlier);
  - ∅ ("never succeeds") on a definition that a run completes.
- **Handler providers return random values that satisfy the assumed spec,** so a ⊙ claim is
  tested under its own assumption.

**Pass conditions (frozen before the code):**
1. **Soundness:** 0 contradictions at seed 1: 300 random loop programs, 200 random handler
   programs, and every well-formed template mutant, each with 300 interpreter runs.
2. **Teeth:** each of three prover faults, chosen now and injected one at a time, gives at least
   one contradiction at the same seed. Otherwise the fuzzer is too weak and condition 1 means
   nothing:
   - (a) loop-goal trust propagation off (the p3 bug: an `ensure` ✔ over a refuted invariant);
   - (b) the prover treats every assumed-handler value as 0;
   - (c) the loop exit assumes the invariant at k = HI instead of max(LO, HI).
3. **Coverage, so it isn't testing nothing:**
   - ≥ 80% of random programs well-formed;
   - among loop goals, ✔ and ✗ each ≥ 15%;
   - ≥ 30% of interpreter runs end ok.
4. **Determinism:** two runs at seed 1 give an identical summary (tier counts, contradictions).
5. **Follow-through:** every contradiction found is saved as `tests/probes/fz*.k` with a test.
   Fixes are pre-registered like any other step. Programs, time and the count of
   "prover ✗ or ?, interpreter found nothing" (incompleteness, not a failure) are reported.

### FZ finding 1: the checker can hang (found while smoke-testing the fuzzer)

Random loop program `loop#6` (seed 1) never finishes `kern check`: a pre-loop write plus a
perturbed `when` invariant. The stack (faulthandler after 25 s) is in `find_cex`, prover.py. The
first solver has a 5 s timeout, but the counterexample searches after it (`nice`, `small`,
`opt`) have none, and the retry loop's `nice.check()` runs without end. So a
**refuted** goal can hang the checker. That's a liveness bug, not a soundness one: no tier is
wrong, the answer never arrives.

**Fix (pre-registered):** every solver in `find_cex` gets the same 5 s timeout, and a
non-`sat` answer ends that search, keeping the best counterexample so far. A goal whose
counterexample search times out still shows ✗ if its first model replays, and `?` (unconfirmed)
otherwise, never ✔.
**Pass:**
1. `loop#6` finishes in under 60 s;
2. the full test suite passes, and the S2 digest `3cb0e5efa3b63fb8` is unchanged;
3. `tests/probes/fz1.k` holds the program, with a test that it finishes and its tiers.

### FZ finding 2: a provider that can't supply a value crashes the checker

When a handler declares no `need n > 0`, the built-in `rand.below` provider (`randrange(n)`) raises
`ValueError` for n ≤ 0. `kern check` crashes in its random tests (they catch only `KernError`), and
in the host the call would crash instead of failing atomically. 4 of the first 30 random handler
programs hit it.

**Fix (pre-registered):** the interpreter turns any exception from a provider into a
`KernError` ("the provider of X gave no value for …"): atomic, and reported like a broken spec.
**Pass:**
1. none of the 4 programs crashes `analyse`;
2. a host call that hits it returns `status: error` and leaves the world unchanged (test);
3. the full suite passes.

**Oracle fix (not a Kern change):** two calls to `rand.below n` give two call-site goals with the
same text: the first ✗, the second ✔ (the first call already required `n > 0`). The oracle
blamed both. A contradiction now needs every goal with that text to be claimed. That's weaker,
but it never blames a correct goal. The 3 remaining smoke-run "contradictions" were all this.

### FZ finding 3: ∅ "never succeeds" on code that succeeds (the frozen run at seed 1)

The first full run (677 programs, 664 well-formed, 224,400 interpreter runs, 112 s) found **no ✔ or
⊙ contradiction, and no crash**. It found 2,919 contradictions of one kind, in 10 programs:
`loop#92` and 9 mutants of p3 and p7. Each shows ∅ on a definition the interpreter runs to
success. In p7 the page says `liar … Never succeeds` and, two lines later, `random tests 500/500
reached success`.
- **Cause:** a refuted loop invariant (p7's `invariant 1 = 0`) makes the prover's after-loop
  state impossible, so every later goal is vacuous. "No input gets past a loop" is true *of the
  model*, where the invariants are assumed, and false of the code.
- **Nothing turned green** (the loop goal is ✗ and the page is red). But the page states a
  falsehood, and the frozen rule counts it.
- **The same reasoning covers** a ∅ that rests on a callee whose spec is broken ("no input
  succeeds given the specs of X"). The fuzzer didn't generate that case.

**Fix (pre-registered):** trust propagation treats a vacuous goal like a proven one. If its
vacuity rests on anything unproven (a loop goal of the definition, a callee that isn't clean, a
store invariant), it becomes `!`, blocked by those, and keeps its reason as a note. ∅ stays only
for "no input passes its own needs", or when everything it rests on is proven. The status line
then follows the remaining goals (✗ for p7).
**Pass:**
1. the 10 programs give 0 contradictions;
2. p7 shows no ∅ and is ✗, through its refuted entry goal;
3. a definition whose own needs contradict each other still shows ∅ (test);
4. the full suite passes, with S2 digest `3cb0e5efa3b63fb8`;
5. the frozen seed-1 run, the three faults and the determinism re-run are all repeated
   after the fix, and all reported.

### FZ finding 4: a hang that didn't reproduce (fault runs)

Two of the three runs with fault (a) injected hung on one program: a worker at 100% CPU for over
30 minutes. The third, with a per-program trace on, finished normally. All three ran the same
code, and fault (a) only changes trust propagation after proving. So the hang depends on state
left behind by earlier programs: pool workers are reused, and Z3's choices depend on earlier
solver calls (a known limitation in CLAUDE.md). **The cause isn't confirmed:** no stack was
captured.

**Changes (pre-registered):**
- **Each program runs in a fresh process** (`maxtasksperchild=1`). Nothing leaks between
  programs, a hang becomes reproducible, and condition 4 (determinism) means something.
- **The last solver without a time limit gets one:** `handler_sat`'s per-value fallback
  (`report.py`), the same 3 s as its main check.
- **The trace stays on for the reported runs** (`KERN_FUZZ_TRACE`). Any program still running
  after 90 s dumps its stack, and that program becomes a probe.

**Pass:** the frozen sequence (seed 1, faults a/b/c, seed 1 again) finishes with no hang. If a
program does hang, it's reported with its stack, not dropped.

### FZ finding 5: counterexample replay can run forever (the cause of finding 4)

With the trace on, fault run (c) caught it: `batch.k:advance_subs` with the mutant
`clock.now → 0`, stuck after 90 s. The stack is `find_cex` → `render_cex` → the interpreter's
loop check (`_check_loop` → a quantified invariant). It's in unfaulted code:
- when the bounded `nice` search gives no answer, `find_cex` replays the first solver's model,
  whose parameters are unbounded (e.g. a loop from −9,725 to 283);
- the replay runs that loop and checks every quantified invariant over the whole range at every
  head. That's quadratic in a huge range, so in practice it never ends.
- **Finding 1's fix made it likelier:** `nice` now answers "unknown" more often, so this fallback
  runs more.
- It depends on which model Z3 picks, hence the non-reproducible hangs in finding 4.

**Fix (pre-registered):** the interpreter takes an optional step budget, counting loop iterations
and quantifier evaluations. A replay uses 200,000 steps. Over budget, the counterexample is
unconfirmed (`?`, "replay too long"), never ✗ and never ✔. Normal runs and the host have no budget.
**Pass:**
1. the mutant finishes in under 60 s (a test, built from `fuzz.template_mutants()`);
2. the full suite passes, with S2 digest unchanged;
3. the whole frozen sequence finishes with no stack dump in the trace.

### FZ results (seed 1, final code: after findings 1–5)

The frozen sequence finished with no hang and no stack dump in the trace: seed 1, faults a/b/c,
then seed 1 again. Each run took ~34 s on 14 workers.
- **Programs:** 677. That's 300 random loop, 200 random handler and 177 template mutants;
  664 well-formed. Each definition got 300 interpreter runs: 224,400 runs, 139,782 ending ok.

| # | condition | result |
|---|---|---|
| 1 | 0 contradictions at seed 1 | **pass:** 0 contradictions, 0 crashes, 0 unmapped evidence |
| 2 | each injected fault is found | **pass:** (a) trust propagation off: 10,461 contradictions in 79 programs; (b) handler values taken as 0: 12,920 in 89; (c) loop exit at HI: 3,415 in 26 |
| 3 | coverage: ≥ 80% well-formed; loop goals ✔ and ✗ each ≥ 15%; ≥ 30% ok runs | **fail as written.** Well-formed 487/500 = 97%, and ok runs 62%, both pass. Loop goals: ✔ 53%, but ✗ only **8%** (187/2,356). A further 36% are `!`, mostly goals resting on a refuted one, so wrong programs are common, but direct refutations are under the bar. The teeth results show the fuzzer can still find these bug kinds |
| 4 | two seed-1 runs, identical summary | **fail as written.** All counts are identical except one program, where one loop goal went ✗ → `!` and one ensure `!` → unknown. Cause: the prover's time limits are wall-clock (5 s), so load decides close cases. Nothing moved to or from ✔ |
| 5 | contradictions become probes with tests; fixes pre-registered | **pass:** findings 1–5, each with a test (`tests/probes/fz1.k`, `test_fz_*`, `tests/test_fuzz.py`) |

**What it found:** 5 bugs the hand-written probes and tests had missed.
- 2 hangs (counterexample search without a time limit; replay of a huge loop range);
- 1 crash (a provider with no value);
- 1 false wording: ∅ "never succeeds" on code that succeeds, also in the F5 callee case;
- the non-reproducible hang, which was the second hang's symptom.
- **None was a wrong ✔ or ⊙.** After the fixes, nothing on any page claimed a guarantee the
  interpreter could break.

**Recommendations:**
- **Determinism:** use Z3's resource limit (`rlimit`, counted in solver steps) instead of
  wall-clock timeouts. The same input then gets the same answer whatever the load. That changes
  the prover's budget everywhere, so it needs its own step, measured against C6.
- **Coverage:** add generator templates that refute directly, e.g. an off-by-one bound in the
  invariant only, not also in the ensure.
- **Loops' status is Fredrik's call** ("a proposal until the fuzzer has run clean"). The soundness
  and teeth conditions pass; two conditions about the fuzzer's own quality fail as written.
  Loops stay a proposal, and the host keeps its runtime checks on, until he says otherwise.

## L: loops, round 2 (2026-11-04, asked by Fredrik: "lets work on those loops")

Order:
- **L1 determinism** and **L2 fuzzer coverage** first: they close FZ's two failed conditions.
- Then packages and imports (**P**).
- Then the loop language gaps the new example apps actually need (**L3**).

Whether loops stop being a proposal stays Fredrik's call once these are reported.

### L1: deterministic solver limits (pre-registered)

**Why:** FZ condition 4 failed. One program's tiers differed between two runs of the same seed,
because the prover's limits are wall-clock (5 s, 3 s): machine load decides close cases. Z3's
`rlimit` counts solver steps instead, so the same query gets the same answer whatever the load.

**Change:** every `set("timeout", …)` in prove.py and report.py becomes `set("rlimit", N)`, with
N calibrated by measurement. Z3's `rlimit count` statistic is recorded for every check on the
10 example modules and the probes. N is set well above the largest count of any check that
finishes, and the rlimit that the current 5 s timeout corresponds to on this machine is
reported.

**Pass conditions (frozen before the code):**
1. no tier changes on the 10 example modules: the S2 digest `3cb0e5efa3b63fb8` holds (if it moves,
   a dated note says which goals and why; the digest test isn't silently updated);
2. C6 `--quick` re-measured against its frozen thresholds (P1 ≤ 1 s per definition at d=6, k=4, e=6;
   P2 ≤ 60 s for n=200 at d=3, k=2, e=3; `--quick` was 0.36 s and 6.8 s after S2), and reported;
3. FZ condition 4 re-run: two seed-1 fuzzer runs give identical summaries;
4. the full test suite passes.

### L2: fuzzer coverage, FZ2 (pre-registered)

**Why:** FZ condition 3 failed (it stays failed as written). Only 8% of loop goals were ✗,
against a bar of 15%. The cause is structural: one wrong invariant among two or three gives one
goal ✗, and the others rest on it and become `!`.

**Change:** `gen_loop` gets a mode per program, chosen at random:
- **mixed:** as before;
- **invariant-only:** a single body statement and its single correct invariant, with only the
  invariant perturbed; the body and the ensure stay correct;
- **ensure-only:** correct invariants, with only the ensure perturbed.

The oracle, the handler generator and the template mutants are unchanged.

**Pass conditions (frozen before the code), all at seed 1, the same sizes as FZ:**
1. 0 contradictions and 0 crashes;
2. each of the faults a, b and c gives at least one contradiction;
3. ≥ 80% of random programs well-formed; among loop goals, ✔ ≥ 15% and ✗ ≥ 15%; ≥ 30% of runs ok;
4. two seed-1 runs give identical summaries (after L1);
5. the trace shows no stack dump.

## P: packages and imports (2026-11-04, asked by Fredrik). A proposal

**Why:** Fredrik asked for "example applications with some package structure, imports and other
normal stuff used in a program". Kern has had one file per module and no imports.

**Proposed design, with token costs measured (`kern.views.tokens`):**

| form | chosen | tokens | alternatives (tokens) |
|---|---|---|---|
| import a module | `import billing.invoice` (column 0, before the definitions) | 4 | `use billing.invoice` 4 (reads like the inferred `uses`), `with …` 4, `from … import a b` 7+ |
| bring in names | every definition of the imported file, unqualified | 0 | `import X: a b` +3 per line; qualified `X.a` +2 per call, and it collides with `ledger.debit` / `hold.get` / `rand.below` |

- **A package** is a directory with a `kern.pkg` file (it may be empty). Loading any file in it
  loads every `.k` file under it, so callers in other files are always known (`kern_callers`,
  re-checks after an edit).
- **A module** is one file. Its name is its dotted path from the package root:
  `billing/invoice.k` is `billing.invoice`.
- **Outside a package,** a file loads itself plus whatever it imports, resolved from its own
  directory. Every existing single-file example is unchanged.
- **Import controls visibility, not naming.** A file may call (in body or spec) only
  definitions it declares or imports directly, and may use only state, handlers and channels
  declared in a file it imports. Names stay unique across the package: a duplicate is an error
  naming both files. The prover, interpreter and host still see one merged module; each
  definition records the file it came from.
- **Invariants** belong to the file that declares their state, and hold package-wide (they did
  module-wide before).
- **Cycles** are allowed: visibility is per file, and nothing is evaluated at load time.
- **Isolation:** module names are identifiers joined by dots, so an import can't name `..` or an
  absolute path. The MCP server's root check applies to every file it loads.
- **Tools:**
  - `check FILE` checks the whole package, and the page groups definitions by file.
  - The MCP edit tools write to the file that holds the definition; `kern_add` takes the target
    file.
  - The host serves a package.
  - The ratchet baseline and the `.why` store are per package (`<root>/kern.strength`,
    `<root>/kern.why`), or per file outside a package, as before.

**Pass conditions (frozen before the code):**
1. **Loader:** a package of several files loads into one module, each definition knowing its
   file. These are errors with file and line: an unknown module, a name used but not
   imported, a duplicate name across files, and `import ../x`.
2. **Tools:**
   - `kern check` on any file of a package checks the package;
   - `kern view NAME` works across files;
   - the MCP tools list, view and check a package;
   - `kern_replace` writes the right file, and re-checks callers in other files;
   - the host serves a package (a test call through it).
3. **Floors:** every existing example and test is unchanged (S2 digest), and single-file
   behaviour is identical.
4. **Docs:** REFERENCE.md stays within 2,500 tokens (measured; if the budget binds, Fredrik
   decides); GUIDE.md covers packages; test_docs passes.
5. **Example apps:** three packages of 3–5 files each, with imports, using loops and/or assumed
   handlers. Each is clean, has examples, and is run through the host. Every place where "normal
   program stuff" was missing (records, strings, constants, collections, …) goes in a gap
   table: what was needed, the workaround, and its token cost. Nothing beyond imports is
   added without Fredrik's say.

**Correction (2026-11-04):** after FZ finding 3 I reported "the S2 digest test passed". There is
no such test. The digest was a one-off script in the overnight run, and only the suite had
passed. The script is now saved, recovered verbatim from the session transcript, as
`validation/digest.py`. It was re-run after L1, on every change of the day (the FZ fixes, the vacuity
rule, the replay budget, rlimit), and gives **`3cb0e5efa3b63fb8`, unchanged**. So the claim
holds, but it hadn't been checked when I made it.

### L1 results

**Calibration:**
- 5,265 solver checks over the 14 example modules, the 8 probes and C6's P1/P2 (`analyse`,
  no mutation testing).
- Every check that finishes uses ≤ 85,675 Z3 steps (p50 194, p99 12,294), all under 15 ms.
- One check hit the old 5 s timeout (fz1's counterexample search). It used 15.5M steps in those
  5 s, about 3.1M steps per second on this machine.
- **`RLIMIT = 10,000,000`:** 116× the largest finishing check, about 3 s here.
- Z3's per-solver `rlimit` turned out to be relative, not global: fz1 run three times in one
  process took 2.8–4.3 s each, with identical tiers.

| # | condition | result |
|---|---|---|
| 1 | no tier changes, S2 digest | **pass:** `validation/digest.py` gives `3cb0e5efa3b63fb8` |
| 2 | C6 `--quick` against its thresholds | **pass:** P1 0.354 s per definition (≤ 1 s), P2 6.58 s (≤ 60 s); 10/10 and 200/200 clean |
| 3 | FZ condition 4: two seed-1 runs identical | **pass:** identical summaries (0 contradictions). The two runs used 12 and 6 workers, the digest ran alongside, and they took 31.5 s and 58 s. No stack dumps |
| 4 | full suite | **pass:** 315 passed, 2 xfailed |

### FZ findings 6 and 7 (found by FZ2's determinism condition, after L1)

FZ2 condition 4 failed even with step limits: one ensure was unknown in one seed-1 run and ✗ in
the other.

**Finding 6: results depended on Python's hash seed.**
- Every fresh process gets a random string-hash seed. The prover iterated sets (the stores a loop
  writes, its channels) in hash order and created Z3 arrays with fresh names in that order, so the
  same program produced a differently-ordered query.
- Run with fixed seeds, every program repeats exactly (seed 1 twice: 0 of 677 differ). Seeds 1 and
  2 differ on 3 programs.
- **So CLAUDE.md's "CLI runs are reproducible" was wrong:** two `kern check` runs of the same file
  could differ on close cases.
- **Fix:** those iterations are sorted (`prove.py`, `_havoc` and `exec_loop`). After it, the 3
  programs are identical under hash seeds 1–4.

**Finding 7: a ✔ resting on an unproven sibling invariant.** This one is the real bug that finding
6 exposed. In `loop#249`, under one seed, the `aux` invariant's "kept" goal has a counterexample
that doesn't replay, so it's `?`; propagation then turns it into `!`. A "kept" goal assumes *all*
the loop's invariants at the head, but `weak_for` doesn't count a sibling that's `!` as a blocker.
Once the `?` became `!`, the module invariant's "kept" goals were left ✔, although their induction
step assumed the unproven `aux` invariant. The oracle couldn't see it, because that invariant
happens to be true. It's an unjustified ✔, not a false one.
- **Fix (pre-registered):** a loop-derived goal is blocked by every sibling loop goal not ✔ or ⊙,
  `!` included. A circle of `!` goals is fine: it has a root, the refuted, unconfirmed or unknown
  goal that started it.
- **Pass:**
  1. `loop#249` shows no ✔ "kept" goal under any of hash seeds 1–4 while its `aux` "kept" goal
     isn't ✔ (a test, `tests/probes/fz2.k`);
  2. the full suite passes; `validation/digest.py` holds or is explained;
  3. per-program tiers are identical under hash seeds 1 and 2 for all 677 programs;
  4. the FZ2 sequence is re-run and reported.

### FZ2, first run (before findings 6 and 7), as it came out

- **Conditions 1, 2 and 5 pass:** 0 contradictions, 0 crashes and no stack dump in 677 programs.
  The faults: (a) 9,024 contradictions in 63 programs, (b) 15,053 in 92, (c) 2,986 in 24.
- **Condition 3 fails again:** ✗ is still **8.1%** of loop goals (✔ 55.7%, well-formed
  488/500, ok runs 64%). The new modes didn't move it. A program with one wrong invariant gives
  one ✗ "kept" goal out of about four loop goals: its "holds when the loop starts" goal is
  usually ✔, and the module invariant's goals rest on the ✗ one. And the ensure-only programs
  have only ✔ loop goals. So the share is capped by structure. More templates would chase the
  metric, not the fuzzer's ability to find bugs, which condition 2 measures directly.
- **Condition 4 fails:** see findings 6 and 7 above.

**Generator fixes found along the way** (they change which programs are well-formed, not the
oracle):
- the constant perturbation could hit a map key (`tally.get' 0` → `tally.get' -1`, which
  doesn't parse);
- one handler ensure template read the after-state in a `when` condition, which is rejected.
  It was already there in FZ.

### FZ2 results (final code: findings 6 and 7 fixed, generator fixes)

Finding 6 needed one more fix: `_loop_effects` iterated a callee's `writes` set. After it, **0 of
676 programs differ between hash seeds 1 and 2** (per-program tiers).

| # | condition | result |
|---|---|---|
| 1 | 0 contradictions, 0 crashes | **pass:** 677 programs, all well-formed, 228,300 runs, 0 contradictions, 0 crashes, 0 unmapped |
| 2 | each fault found | **pass:** (a) 9,428 contradictions in 66 programs, (b) 15,048 in 92, (c) 3,248 in 26 |
| 3 | coverage | **fail as written:** well-formed 100% and ok runs 64% pass; loop goals ✔ 53.6%, but ✗ **8.2%** (bar 15%). It's structural, as explained under FZ2's first run |
| 4 | two seed-1 runs identical | **pass** (and identical per program under different hash seeds) |
| 5 | no stack dump | **pass** |

Also: the full suite (317 passed, 2 xfailed); `validation/digest.py` gives `3cb0e5efa3b63fb8`.

**Where loops stand, for Fredrik:**
- Soundness, teeth and determinism all pass now.
- The ✗-share bar fails twice, for a structural reason, not a weak fuzzer: the faults are found
  in 24–92 programs each.
- Since the first run, the fuzzer has found 7 bugs:
  - 2 hangs, 1 crash, 1 false "never succeeds";
  - the non-reproducible hang (from replay);
  - hash-order results;
  - and finding 7, a ✔ resting on an unproven sibling invariant.
- None was a ✔ on a guarantee the interpreter could break, but finding 7 was a ✔ the proof didn't
  justify.
- Still missing from loops:
  - a stated frame (the `lo − 1` / `hi + 1` mutants survive);
  - sum;
  - events in loops;
  - nesting.

### P results

| app | files | definitions | tokens | loops | handlers | clean | `check --quick` |
|---|---|---|---|---|---|---|---|
| `examples/shop/` (money, inventory/catalog + promotions, orders) | 4 | 7 | 969 | 2 | 0 | 7/7 | 1.9 s |
| `examples/library/` (money, catalog/books, loans, fines) | 4 | 6 | 1,054 | 1 | 0 | 6/6 | 1.0 s |
| `examples/lottery/` (money, chance, tickets, draw) | 4 | 4 | 780 | 1 | 1 | 4/4 | 0.8 s |

| # | condition | result |
|---|---|---|
| 1 | loader and its errors | **pass** (`tests/test_package.py`): one module per package, each definition with its `.module`, `.file` and line. Errors name file and line: an unknown module, a name used but not imported (with the import to add), state from a file not imported, a duplicate across files, and `import ../x` / `/etc/passwd` / `Money`, which can't name anything |
| 2 | tools | **pass:** `check` on any file or the directory checks the package, with a `== module (file)` line per file; `view` works across files; MCP list, view and check; `kern_replace` writes the defining file, refuses the wrong file (naming the right one) and re-checks callers in other files; an edit that breaks an import is refused with the file unchanged; the MCP root covers every file of the package; the host serves all three packages, and the lottery draw replays exactly |
| 3 | floors | **pass:** single files load exactly as before (same build hashes, no package attribute); the full suite passes (342 passed, 2 xfailed); `validation/digest.py` gives `3cb0e5efa3b63fb8` |
| 4 | docs | **pass:** REFERENCE.md 2,126 → **2,230** tokens (≤ 2,500); GUIDE.md "Packages: several files", the API row and the files list; test_docs passes |
| 5 | example apps | **pass:** three packages, all clean, with loops and an assumed handler, all served by the host (tests). The gap table follows |

**Found while building them:**
- **The library's first `borrow` was wrong,** and the prover said so. `due = clock.now + 14` breaks
  `due ≥ 0` when the clock is negative, which Kern doesn't rule out (counterexample `now:-15`).
  The fix is an explicit `need clock.now >= 0`. Until then `assess` was `!`: it relies on the `due`
  invariant, which only holds if every writer is proven. That's sound, and it's how one file's bug
  shows up in another's page.
- **The `!` lists repeated blockers after finding 7;** they're now deduplicated.
- **The `uses` line still followed the hash seed:** effects outside the fixed order kept the set's
  order. It's now sorted by name.

**Gaps: "normal program stuff" these apps needed and Kern doesn't have** (nothing added; for
Fredrik):

| needed | where | workaround now | cost |
|---|---|---|---|
| an **Account stored in state** (values are Int) | a loan's member; a ticket's owner | `pay_fine` takes the member as an argument and can't check it's the borrower; `draw` picks among 3 fixed accounts | a guarantee that can't be stated ("only the borrower pays") |
| **iteration over accounts / collections** | weighted draw over all ticket holders; payroll over employees | none: loops range over Int keys | the feature can't be written |
| **multiplying two variables** (`price × qty`) | buying several units | `buy` sells exactly one unit | the operation has to be called n times |
| **named constants** | ticket price 10 (3 times in `buy_ticket`), loan period 14 (2), fine 5 (4 in `assess`) | literals repeated in spec and body | a changed constant has to be changed in each place; a missed one is caught by the prover only where a spec states it |
| **an effect that returns a value** (only pure functions return) | a new loan id or ticket id | the caller supplies the id; `borrow` refuses `LoanExists` | the caller must find a free id |
| **a write at a computed key** keeps invariants provable | returning a loan's book (`copies` at `lent.get loan`) | `give_back` takes the book as well and checks `lent.get loan = book` | one redundant parameter and one need |
| **a stated range for `clock.now`** | due dates | `need clock.now >= 0 else BadClock` | one need per definition that writes a time |
| **records** | a loan is `due`, `lent`, `fine`: three parallel maps keyed by loan id | parallel `state` maps | no "a loan" type, and its fields can't be kept consistent by type |
| **text** | book titles, member names | none | — |
| **code shared between packages** | each app has its own copy of `money/ledger.k` (169 tokens) | copy the file | imports reach within one package only |

**Correction to FZ2's condition 3 explanation (measured after writing it).** I called the 8% "capped by
structure" without measuring. Per mode, on the 300 random loop programs at seed 1:

| mode | programs | loop goals | ✔ | ✗ | `!` |
|---|---|---|---|---|---|
| mixed | 89 | 430 | 69.5% | 7.7% | 22.8% |
| invariant-only | 110 | 288 | 61.5% | **20.5%** | 18.1% |
| ensure-only | 101 | 503 | 73.2% | 2.2% | 24.7% |

- **The invariant-only mode works:** 20.5% clears the 15% bar on its own, though it's below my ~25%
  prediction.
- **The aggregate stays at 8% by dilution,** not because of a cap:
  - the ensure-only mode keeps its invariants correct by design, so its loop goals are mostly ✔;
  - the 200 handler programs and 177 template mutants add 1,022 more loop goals, with few direct
    refutations.
- **The 2.2% ✗ in the ensure-only mode** shows that some of the generator's "correct" invariants
  aren't: two statements writing the same state in one loop each get the invariant for themselves.
  That's a generator imperfection. Those programs are still wrong programs, and the oracle treats
  them like any other.
- **The condition was a single aggregate bar, and it fails as written.** Measured per mode, the
  directly refuting mode reaches the bar.

**What the S2 digest does and doesn't cover.** Its ten modules (escrow s1–s3, ledger v3–v7,
ledger_large, one C6 configuration) have **no loops, no handlers and no vacuity**. So "digest
unchanged" after FZ findings 3 and 7, L1 and P shows only that those changes left non-loop code
alone. The loop and handler paths they changed were checked by the probe and fuzzer tests alone.
From now on, a second digest covers them: `validation/digest.py --loops` over batch.k, raffle.k, p1–p7,
fz1, fz2 and the three packages (statuses, confirmation and clean flags, no mutation testing).
Its baseline, recorded after findings 1–7 and P, is **`64051b1d363a6553`**, the same under hash
seeds 1 and 2. The S2 value stays a frozen reference.

**Also fixed:** the host looked for `<file>.host` next to the file passed. In a package it now
reads `<root>/kern.host`. And the result cache is per directory (`.kern-cache` next to the file
passed), so a package can end up with a cache per subdirectory. That's conservative, since the
key covers everything, but it's duplicated work.

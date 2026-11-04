# Roadmap run (started 2026-11-03, overnight)

Fredrik asked for work on each roadmap item: example applications to test and evaluate
continuously, reviews and second opinions from session kern-90, and staying true to
FUNDAMENTALS.md. Each step's pass conditions are written here **before** its code. Results
are appended under each step, failures included.

Git is blocked (the Xcode licence prompt). Every step is checkpointed in `.checkpoints/` (see
its LOG.md) so it can be committed in order later.

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
   is still 13/13; TS and Go are still 13/24 each. Raw data: `challenge/c1/results_after_S1.json`.
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
6. **C6 re-measured** (thresholds unchanged; `challenge/c6/results_after_S2.json`):

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

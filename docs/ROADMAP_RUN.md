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
   **24/24** expressible live bugs (it was 23/24). B10 is now caught by the ratchet alone. P1
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

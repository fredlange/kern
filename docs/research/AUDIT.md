# Audit of the Kern prototype against its fundamentals

Date: 2026-11-03. Code state: the baseline commit `0d49b84`, plus the staged `emit`
feature (ledger v4), plus this audit's rendering fixes. Fundamentals:
[FUNDAMENTALS.md](../FUNDAMENTALS.md). Tests: `tests/test_fundamentals.py`
(`.venv/bin/pytest`). Current result: **37 passed, 23 xfailed**. Every xfail is
`strict`, so it turns into a failure (XPASS) the day its gap is closed.

Each probe was run, not argued. Numbers come from the commands or tests named.
Token counts use the `anthropic==0.34.2` tokenizer (an older one, so approximate).

## Status after the fix pass

Every suggested fix has been made; [FIXES.md](FIXES.md) has each step with its
before/after evidence. Tests: **97 passed, 2 xfailed** (was 37 passed, 23 xfailed; 81/2 after the
fix pass, 87/2 after the clock, 97/2 after `ok otherwise`).
The two remaining strict xfails are tier ○ and record/replay, both roadmap step 4.

| # | At audit | After fixes | What closed it | What remains |
|---|---|---|---|---|
| F1 | partial | **holds** | lifted refusal conditions (`body need`) in views, prover and page | conditions after the first write that read balances aren't liftable; views show only own body needs (inherited ones ≈ 2× tokens) |
| F2 | violated | **holds** | well-formedness pass (no shadowing, no recursion); in-place rollback on any exception | recursion is rejected rather than supported |
| F3 | partial | **holds** (opt-in) | inferred write set on page and views; "no guarantee about the new balance of …"; baseline flags removed guarantees; `ok otherwise` (FIXES.md) proves a call refuses only as its spec says | only for definitions that opt in; callers restate callee refusals (+66 tokens for payout) |
| F4 | holds / partial | **holds** | pure specs; bad calls rejected at the boundary; every valid outcome is ok or an inferred failure | — |
| F5 | partial | **holds** (except ○) | tiers ∅ ? ◐; strength headline = guarantees; ✗ only when replayed | ○ runtime-checked (roadmap 4) |
| F6 | partial | **holds** | interface-anchored why; orphans reported; obligation lint; `content_hash` | the lint is a keyword heuristic |
| F7 | holds / partial | **partial** | static name resolution; types | record/replay (roadmap 4) |
| F8 | partial | **holds** | ill-formed mutants excluded; payload mutants; baseline | 150 random runs per mutant is a cost trade-off |
| F9 | holds | **holds** | — | `writes` (+4) and `body need` (+20 where present) raise view tokens; measured in FIXES.md |

**Correction to this audit:** F2 below says rollback on declared failures holds.
It didn't. `Interp.run` returned a snapshot copy, but the caller's own world object
stayed half-mutated, and the test only checked the returned value. That was found
while planning the fix and fixed in step 2.

## Summary (at audit time)

| # | Fundamental | Status | Evidence | Gap | Suggested fix | Effort |
|---|---|---|---|---|---|---|
| F1 | Optimise for the reader | **partial** | `test_f1_editor_view_never_contains_a_callee_body` passes. `test_f1_spec_view_says_when_each_failure_happens` xfails | Body-level `need`s (e.g. `Insufficient`) are reachable behaviour that no view shows; callers see the name only | Lift body-level failure conditions into the spec (prove or require `need` in spec), or show them in the spec view as "refuses when (body)" | S–M |
| F2 | Verification licenses not reading | **violated** (soundness) | `test_f2_shadowed_parameter_cannot_fake_a_proof`, `…self_recursion…`, `…mutual_recursion…`, `…runtime_error_mid_body_is_atomic` all xfail; the rollback tests pass | ✔ with wrong code: let-shadowing and recursion both yield proofs of false guarantees; mutual recursion crashes `check`; runtime errors break atomicity | Well-formedness pass before proving (no shadowing, no unknown names, no recursion without a measure); restore the snapshot on any exception | S (shadow, atomicity), M (recursion) |
| F3 | Spec is the primary artifact | **partial** | `test_f3_guarantees_reveal_extra_writes`, `test_f3_removed_guarantees_are_surfaced` xfail | No frame conditions, so a body that also moves money between two other accounts is green. No guarantee diff between builds | Frame clauses (`only ledger.balance from, to`) or an implicit "nothing else changes" frame; a guarantee diff on the page | M–L |
| F4 | Derive, don't declare | **holds** for effects/failures; **partial** overall | `test_f4_observed_effects_and_failures_are_inferred` (7 defs × 300 random runs, observed ⊆ inferred, every inferred failure seen); 4-deep chain test passes; `…every_outcome_is_ok_or_an_inferred_failure` and `…effectful_spec_is_rejected_cleanly` xfail | Runtime errors (unknown account) are outcomes outside `fails`; spec clauses aren't checked for purity | Make "unknown account" a declared failure or impossible by typing; reject effectful handlers in spec clauses | S |
| F5 | Evidence over assertion | **partial** (4 rendering bugs fixed) | Fixed: zero-guarantee label, timeout label, unconfirmed-refutation wording, failure attribution, clause printing. Xfail: vacuity, unconfirmed ✗, ◐/○ tiers, spec strength measuring examples | Green badge on vacuous specs; ✗ "Broken guarantees" on correct code; spec strength unaffected by deleting every guarantee | Vacuity check (is the success path satisfiable?); "unconfirmed" status for refutations that don't replay; report kills per layer | S–M |
| F6 | Separate knowledge by role | **partial** | Body change → stale passes; `eg` change → not stale (documented decision); orphan, callee-interface, obligation-in-prose and hash-with-eg tests xfail | Renamed/deleted definitions orphan why entries silently; a callee change that alters the caller's interface doesn't mark the caller's why stale; rule 3 is unchecked | Report orphans in `check`; hash the inferred interface too (or a separate iface hash); lint why entries for modal words | S |
| F7 | Closed world of effects | **holds** for effects; **partial** overall | `test_f7_body_cannot_reach_python` (4 escape attempts → `KernError`); xfail: unknown names not static, Python value semantics, no replay | Values are Python values (`from + to` = `'AB'`); unknown names are caught only at run time; replay not built | Type checker (roadmap 1); a static name-resolution pass; record/replay (roadmap 4) | M |
| F8 | The ratchet loop | **partial** | Survivors identify the missing guarantee (v3 → v4: 3 → 0) passes; counterexample → valid replaying `eg` passes; ill-formed-kill and payload-mutant tests xfail | 1 of v3 transfer's 10 kills is an ill-formed mutant; failure payloads are never mutated; the "only gets stronger" half isn't tracked | Exclude ill-formed mutants (well-formedness pass); add payload mutants; store a per-definition strength baseline and flag drops | S |
| F9 | Tokens shape syntax, but loading matters more | **holds**, with a leaf caveat | 10 keywords are 1 token each; editor view is constant as the module grows 4× (test) and 8× (measured); chain test passes | For leaf definitions the editor view costs 9–14% *more* than the source | Drop the inferred lines when nothing is skipped, or accept it as the price of the checked interface | S |

## Details per fundamental

### F1. Optimise for the reader
**Implemented by:** `kern/views.py` (`iface`, `spec`, `editor`), `kern/views.py` `tokens`.

- **Holds:** for all 7 definitions in `examples/ledger_large.k`, the editor view
  contains no line of any callee's body
  (`test_f1_editor_view_never_contains_a_callee_body`).
- **Gap:** transfer can refuse with `Insufficient` because of
  `need have >= amount` in its *body*. The spec view shows `fails … Insufficient(Int)`
  but not the condition, so a caller reading the spec can't tell when its call will
  fail. The prover is also blind to it: modular calls assume only *spec* needs
  (`kern/prove.py:93` `modular_call`). That makes `payout` and `rebalance`
  counterexamples land on inputs the body refuses (see F5).
  Test: `test_f1_spec_view_says_when_each_failure_happens` (xfail).

### F2. Verification licenses not reading
**Implemented by:** `kern/prove.py` `modular_call` (callee spec plus inferred `uses`,
never the body), `Prover.prove`, `kern/report.py` trust propagation,
`kern/run.py:121` `Interp.run` (atomic snapshot).

- **Holds: the prover doesn't read callee bodies.** Replacing transfer's body with
  `ledger.credit from 999; audit.emit (Junk clock.now)` (same inferred `uses`) leaves
  payout's proof unchanged (`test_f2_caller_proof_does_not_read_callee_body`).
  Caveat found while writing the test: the prover *does* depend on the callee's
  inferred `uses`, which is computed from the body. That is the interface layer
  (rule 1), but it means "spec only" really means "spec + inferred interface".
- **Holds: rollback on hidden callee failures.** When payout's second transfer hits
  the body-level `Insufficient`, balances *and* the audit log equal the snapshot
  (`test_f2_hidden_callee_failure_rolls_back_whole_call`). **Correction:** this
  checked only the returned value; the caller's own object stayed mutated (fixed in
  FIXES.md step 2). 2,000 random payouts (some
  refused with `Insufficient`) show no guarantee violations
  (`test_f2_modular_proof_agrees_with_execution_on_hidden_failures`). The
  atomic-rollback argument is sound for declared failures.
- **Violated: let-shadowing.** `kern/prove.py:139` rebinds `env` for `let`, and goals
  are evaluated in that env (`:143`). The interpreter's `holds` uses the parameters.
  In `sneaky`, the body does `amount = amount + 1` and then credits `amount`; the
  `ensure … + amount` is ✔ proven, and with `need amount > 2000` random tests never
  succeed (0/500). **Badge: "All guarantees proven".** The only signal was 3 mutation
  survivors (2/5). The interpreter confirms the body credits 3001 for amount 3000
  (`test_f2_shadowed_parameter_really_violates_the_spec`).
- **Violated: recursion.** `loop a` with body `loop a` and
  `ensure balance' a = balance a + 1000` is ✔ proven. The modular call assumes the
  definition's own ensure, and with no write effect (no havoc) the path condition
  becomes `bal(a) = bal(a)+1000`, which is false, so every goal is vacuously true.
  `uses(loop)` is `{}`. Adding `ledger.credit a 1` makes the prover refute it, then
  crash with `RecursionError` while replaying the counterexample.
- **Crash: mutual recursion.** `ping`/`pong` crash `check` with `RecursionError` (the
  proptest only catches `KernError/KeyError/TypeError`).
- **Violated: atomicity on runtime errors.** `Interp.run` restores the snapshot only on
  `Fail` (`kern/run.py:127`). `t A Z 10` (unknown account `Z` after the debit) raises
  `KernError` and leaves `A: 90`. Rule 7 ("a refusal rolls back the whole call")
  holds only for refusals.

### F3. The spec is the primary artifact
**Implemented by:** the proof page (`kern/render.py` `card`) and the spec view.

- **Gap: no frame conditions, so the guarantee list isn't a complete description.**
  Two consequences, from one root:
  - *False green:* a 5-parameter transfer that also does `ledger.debit x 1;
    ledger.credit y 1` (x, y distinct from from/to) is ✔✔✔ "All guarantees proven".
    The page's "Can touch: the ledger (read and write)" is true but doesn't say
    *which* accounts. Spec strength 20/22 (the two survivors are the only hint).
  - *False red:* see F5, `rebalance`.
- **Gap: removal isn't surfaced.** Deleting both balance ensures and the `keep` from
  v4 transfer leaves it green, and spec strength stays 13/13 (F5). Nothing compares
  guarantees between builds, so a reviewer looking at the new page can't tell a
  guarantee vanished.

### F4. Derive, don't declare
**Implemented by:** `kern/infer.py` `called_names`, `uses`, `fails`.

- **Holds (measured):** instrumenting every handler and running each of the 7
  definitions in `ledger_large.k` 300 times at random gives observed effects ⊆
  inferred `uses` and observed failures ⊆ inferred `fails`. Every inferred failure
  was observed at least once, so there is no over-approximation on this module. The
  4-deep chain `month_end → settle → payout → transfer` infers
  `{ledger.write, clock, audit}` and all 5 failures. A handler inside a constructor
  inside a failure payload (`need n > 0 else Late (clock.now)`) is inferred.
- **Gap:** a call can end in ways outside `fails`. An unknown account raises
  `KernError`, which isn't a declared failure.
- **Gap:** spec clauses aren't checked for purity. `ensure ledger.debit a 5` parses,
  then crashes the prover with a `Z3Exception`. In the interpreter it would mutate
  the pre-state copy. Effects in specs are not in `uses`.
- **Over-approximation, safe:** a `let` or parameter that shadows a definition name
  makes inference count the definition's effects anyway (it can only over-claim).

### F5. Evidence over assertion
**Implemented by:** `kern/render.py` (`_status`, `card`, `terminal`), trust
propagation in `kern/report.py`, `kern/prove.py` `render_cex` / `find_cex`.

- **"Can touch … nothing else is reachable":** true *by construction*, not enforced.
  Inference and the interpreter resolve names in the same order (handlers, then
  definitions, then constructors), and every escape attempt fails (F7). There is no
  runtime capability check, so it holds only as long as the AST interpreter is the
  only runtime.
- **Tiers ◐ tested-only and ○ runtime-checked-only: not implemented.** `ICON` has
  only ✔ ✗ ! ?.
- **Z3 timeout ("unknown"):** shown as `?`; the definition isn't clean, mutation
  isn't measured, and callers are downgraded. That counting was right. The label was
  wrong ("Depends on unproven code"), and is **fixed** to "Not proven: prover gave
  no answer" (`test_f5_prover_timeout_is_labelled_as_such`).
- **Zero guarantees was labelled "All guarantees proven".** **Fixed** to "No
  guarantees stated" (warn).
- **Vacuity (gap):** `need amount > 0` together with `need amount < 0` makes every
  ensure ✔. The page shows "0/500 reached success" but still says "All guarantees
  proven".
- **Unconfirmed refutations shown as broken (gap).** `rebalance` (withdraw, then
  deposit) is correct (207 random successes, 0 violations) but is shown ✗ "Broken
  guarantees". The counterexample `eg A:0@u1 B:357@u2 | rebalance A B 203 ->
  Insufficient 203` doesn't even succeed. The cause is no frame conditions plus
  body-level failures (F1, F3). The wording is **fixed**: the page and terminal now
  say "Not reproduced in the interpreter". The *status* is still ✗ (design-level).
- **Spec strength mostly measures examples (gap).** Per-layer kills, measured
  independently on every mutant:

  | Definition | Mutants | Killed by examples | Random tests | Prover |
  |---|---|---|---|---|
  | v3 transfer | 13 | 10 (1 is a crash) | 7 | 6 (+1 prover crash) |
  | v3 payout | 12 | 12 | 6 | 6 |
  | v4 transfer | 13 | 10 (1 crash) | 10 | 9 (+1 crash) |
  | v4 payout | 12 | 12 | 8 | 8 |

  Headline: **v4 transfer with every `keep` and `ensure` deleted still scores 13/13.**
  The proven guarantees add nothing to the number the page calls "spec strength".
- **Rendering bugs fixed** (each is covered by a passing test):
  1. `code()` and `en()` dropped parentheses: `a - (amount - fee)` printed as
     `a - amount - fee`, which reads as a different guarantee. Now
     `parse(code(e)) == e` (`test_f5_printed_clauses_mean_what_was_written`).
     `kern/syntax.py:287` `_side`.
  2. "Refuses when": inherited failures were the union over all callees, listed
     under every callee (rebalance's page said `deposit` refuses with `Insufficient`).
     Now attributed per callee.
  3. Status label for zero guarantees (above).
  4. Status label for prover timeouts (above).
  5. Unconfirmed counterexamples now say "Not reproduced in the interpreter".

### F6. Separate knowledge by role
**Implemented by:** `kern/why.py` (`load`, `status`, `anchor`),
`kern/syntax.py:180` `Def.hash`.

- **Holds:** a body change marks the why entries stale.
- **`eg`-only change doesn't mark why stale.** `Def.hash` excludes examples. *Should
  it?* For staleness I'd say no: examples are evidence, not semantics, and the test
  records that as the current decision. For content addressing (roadmap 2), though,
  two definitions that differ only in examples get the same hash, so a hash-keyed
  store would merge them (xfail). These are two different hashes wanting to be one.
- **Callee changes are invisible to the caller's hash.** From v1 to v2, payout's
  inferred interface gains `SameAccount`, but its hash stays `d8d99a9c`. The README
  said payout's why was flagged stale in v2; that was false and is **corrected** in
  README.md. Design question: should a why anchor cover the inferred interface?
- **Orphans:** renaming `transfer` to `move` silently hides its 2 why entries.
  `anchor` keeps them under the old name without a warning. `kern why FILE name`
  with an unknown name raises `KeyError`.
- **Rule 3 is unchecked.** The demo's own `ledger.why` says "cross-customer transfers
  **must** go through payments.send". That is an obligation on callers that lives
  only in prose. `need SameOwner` covers half of it (transfer refuses); the "use
  payments.send instead" half can't be expressed.

### F7. Closed world of effects
**Implemented by:** `kern/run.py:53` `HANDLERS`, `Interp.call` (handlers, then
definitions, then capitalised constructors, otherwise `KernError`).

- **Holds for effects.** `__import__ 1`, `os.system 1`, `from.__class__` and `print 1`
  all raise `KernError: unknown name` at runtime. There is no path from a body to
  Python other than the handler table.
- **Gap: unknown names aren't a static error.** `uses()` silently ignores them (`{}`
  for `os.system 1`), and the prover crashes with `ValueError: prover: unknown
  os.system` instead of reporting.
- **Gap: values are Python values.** `audit.emit (Foo (from + to))` emits
  `('Foo', 'AB')`: string concatenation in the interpreter, integer addition in the
  prover. Same root as the shadowing issue in F2: the prover and interpreter don't
  share a checked semantics (there is no type checker).
- **Not implemented:** record/replay (roadmap 4); runtime enforcement of a
  definition's `uses`.

### F8. The ratchet loop
**Implemented by:** `kern/check.py` `mutants`, `mutation_score`;
`kern/prove.py` `render_cex`.

- **Holds:** a counterexample is a syntactically valid `eg` line that replays
  (`test_f8_counterexample_is_a_valid_eg_line_that_replays`). Survivors point at
  the missing guarantee: v3's 3 survivors are exactly the audit mutants, and v4's
  `emit` kills them. Note that the counterexample's `eg` line records the
  *observed* (buggy) outcome. It is a regression input, not an example to paste
  in as is.
- **`except Exception: dead = True` (`kern/check.py:122`)** fired **0 times** in v3
  and v4. The crash kill happens earlier: `drop have = ledger.balance from` leaves
  an unbound name, the example run errors, and that counts as a failed example.
  Inflation: **1 of 10 kills in v3 transfer**. Excluding ill-formed mutants gives
  9/12, not 10/13. The same mutant crashes the prover (`ValueError`), so a
  well-formedness pass would catch it statically.
- **Gap:** no mutant touches a failure payload (`Insufficient (amount - have)`).
  Only the `need` condition is mutated.
- **Gap:** "the spec only gets stronger" isn't tracked. There's no baseline, and
  since strength doesn't fall when guarantees are deleted (F5), the gauge couldn't
  detect a weakening anyway.

### F9. Tokens shape syntax, but loading matters more
**Implemented by:** `kern/views.py`.

- **Keywords:** `need ensure keep uses fails eg why else body emit` are 1 token each
  in context (`test_f9_keywords_are_one_token`).
- **Views on `examples/ledger_large.k`** (7 definitions, 906 tokens, chain depth 4):

  | Definition | Depth | iface | spec | editor | own + transitive callee source | editor / that | editor / module |
  |---|---|---|---|---|---|---|---|
  | transfer | 1 | 30 | 104 | 250 | 230 | 1.09 | 0.28 |
  | deposit | 1 | 18 | 63 | 100 | 89 | 1.12 | 0.11 |
  | withdraw | 1 | 21 | 58 | 117 | 103 | 1.14 | 0.13 |
  | payout | 2 | 37 | 85 | 240 | 346 | 0.69 | 0.26 |
  | rebalance | 2 | 29 | 60 | 220 | 273 | 0.81 | 0.24 |
  | settle | 3 | 41 | 99 | 278 | 515 | 0.54 | 0.31 |
  | month_end | 4 | 42 | 90 | 238 | 630 | 0.38 | 0.26 |

- **Scaling** (k renamed copies of the module; editor view of `month_end`):

  | k | Definitions | Module tokens | Editor tokens | Ratio |
  |---|---|---|---|---|
  | 1 | 7 | 931 | 242 | 0.260 |
  | 2 | 14 | 1,863 | 242 | 0.130 |
  | 4 | 28 | 3,727 | 242 | 0.065 |
  | 8 | 56 | 7,455 | 242 | 0.032 |

  The editor view is O(own definition + direct callees' specs). It is independent
  of module size and of chain depth below the first level. The honest comparison
  is against reading the call chain (the "own + transitive" column), since that is
  what an agent would read without verification. That ratio falls with depth
  (0.69 at depth 2, 0.38 at depth 4) and is **above 1 for leaves**: the editor view
  adds `uses`/`fails`/`why` lines and skips nothing.
- Caveat: synthetic replication measures structure, not a real codebase's fan-out.

## Incidental findings
- **Parser:** `need amount > 0 and amount < 10 else Bad` is a parse error (`and`
  after a comparison). The fix is one rule in `ExprParser.expr`: loop over
  `and`/`or` with comparisons as operands. Not fixed, because it changes the grammar
  (`test_incidental_and_after_comparison_parses`, xfail).
- **`kern why FILE unknown_name`** raises `KeyError`.

## Fixes made in this audit
1. `kern/syntax.py` `code()` and `kern/views.py` `en()`: parenthesise binary
   operands (new `_side`). Hashes are unaffected (they use the AST).
2. `kern/render.py` `card`: per-callee attribution of inherited failures
   (`report.py` now passes `defs` in each report).
3. `kern/render.py` `_status`: labels for zero guarantees and prover timeouts.
4. `kern/render.py` `_how`, `terminal`: "Not reproduced in the interpreter".
5. `README.md`: corrected the v2 claim that payout's why notes are flagged stale.
6. `pytest.ini` (`pythonpath = .`), so `pytest` runs from the repo root.

All v1–v4 results (statuses, spec strengths, hashes, counterexamples) are unchanged.

## Gaps ranked by how much they undermine the approach

1. **The prover and interpreter don't share a checked semantics, so ✔ can be
   wrong with nothing red anywhere.** Let-shadowing (green badge, guarantee false
   for amount > 2000), recursion (proves any ensure), and Python value leaks all come
   from one root: there is no well-formedness or type pass before proving. This hits
   F2 and F5 at their core: a ✔ is the thing that licenses not reading. The
   shadowing and name-resolution parts are small fixes; recursion needs a
   termination measure or a ban.
2. **Spec strength measures the examples, not the guarantees.** Deleting every
   `keep`/`ensure` from v4 transfer leaves it at 13/13. The ratchet's (F8) gauge, and
   the number a human reads as "how much does the spec pin down" (bet 3), don't
   move when the proven part of the spec disappears. Fix: report kills per layer,
   or define strength over the proven guarantees only.
3. **No frame conditions: false greens and false reds.** A body that steals between
   two other accounts gets "All guarantees proven"; the first correct two-callee
   definition written for this audit (`rebalance`) gets ✗ "Broken guarantees". That
   undermines bet 1 (few false alarms) and bet 3 (the guarantee list as a complete
   description) at once.
4. **Vacuous proofs are green.** Contradictory needs show "All guarantees proven".
   Cheap to detect: check that the success path condition is satisfiable.
5. **Atomicity holds only for declared failures.** Any runtime error mid-body leaves
   the world half-mutated; a one-line fix in `Interp.run`.
6. **Body-level failure conditions are invisible to callers and the prover (F1).**
   This is what drives the bad counterexamples.
7. **Why-store integrity (F6):** orphans are silent, callee interface changes don't
   stale callers, and the hash excludes examples. That matters as soon as roadmap
   step 2 content-addresses definitions.

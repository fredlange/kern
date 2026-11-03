# Fix pass after the audit

Fixes for the gaps in [AUDIT.md](AUDIT.md), one step at a time. This file is written
as the work proceeds: each step records what changed, the test evidence, and every
line that moved in the output snapshot.

## Test protocol

**Before any change:** the audited tree was frozen (`before_tree`), and a snapshot
script recorded every observable output: the `pytest` result per test, `kern check`
on v1–v4 and `ledger_large.k` (with exit codes), and 21 `kern view` outputs with
token counts.

**For each step:**
1. Implement the fix.
2. Run `pytest`. The strict xfail that documents the gap must now **fail as XPASS**,
   which proves the test actually discriminates. Only then is the marker removed and
   positive assertions added.
3. Take a snapshot (`after_N`) and diff it against the previous one. Every changed
   line is explained below; a diff that can't be explained counts as a bug.
4. Invariant: `Def.hash` (the AST hash, shown as `[xxxxxxxx]`) must not change in
   steps 1–9.

## Steps

### 1. Parser: `and`/`or` after comparisons
- **Change:** `ExprParser.expr` is now `and`/`or` (left-associative) over
  comparisons over sums. Before, `and` was only accepted after a bare sum.
- **Evidence:** `test_incidental_and_after_comparison_parses` went XPASS, was
  unmarked, and now asserts the parse tree. Added
  `test_incidental_boolean_clauses_print_and_reparse` (5 cases).
- **Snapshot diff vs before:** only pytest counts (37/23 → 43/22). No example output
  changed (none uses `and`/`or`).

### 2. Atomicity: roll back in place, on any exception
- **Audit correction:** the audit marked rollback on declared failures as
  holding. It didn't: `Interp.run` returned a snapshot copy but left **the caller's
  world object** half-mutated. After a refused payout, the caller's `w` showed
  `A: 0, B: 100` and a `TransferDone` event. The original test only checked the
  returned value.
- **Change:** `Interp.run` restores the snapshot into the caller's object (`_restore`)
  on `Fail`, and also on any other exception before re-raising.
- **Evidence:** `test_f2_runtime_error_mid_body_is_atomic` went XPASS and was
  unmarked. `test_f2_hidden_callee_failure_rolls_back_whole_call` now asserts on the
  passed-in object. Run against the pre-fix tree it **fails**
  (`{'A': 0, 'B': 100, 'H': 0} != {'A': 100, ...}`), so it discriminates.
- **Snapshot diff vs step 1:** pytest counts only (43/22 → 44/21).

### 3. Well-formedness pass (`kern/wf.py`, types off)
- **Change:** `check_module` runs before anything is proven or executed (from
  `analyse` and every CLI command). It rejects:
  - unknown names, wrong arity, and a value applied to arguments;
  - a `let` that shadows a parameter, an earlier `let`, a handler or a definition;
  - specs that read body `let`s, call effectful handlers or definitions, use primes
    outside `ensure`, or use `_` outside the top level of an `emit` argument;
  - recursion (call-graph cycles, self-calls included).

  Mutants go through `problems()`; ill-formed ones are listed and excluded from the
  total, not counted as kills.
- **Found while testing:** `infer.callees` leaves out self-calls, so the first cycle
  check missed `loop -> loop` (the test caught it with `RecursionError`). Fixed by
  checking `called_names` for the definition's own name.
- **Evidence:** XPASS on `…effectful_spec…`, `…unknown_names…` and
  `…ill_formed_mutants…`. Shadowing and both recursion tests are rewritten from
  "must not be proven" to "must be rejected with this message". Their old form
  stayed xfail even after the fix, because the exception happened inside the xfail.
  Added `test_f2_spec_cannot_read_body_lets`.
  `test_f2_shadowed_parameter_really_violates_the_spec` (interpreter only) stays, as
  evidence the rejection is needed.
- **Snapshot diff vs step 2 (all explained):** spec strength loses exactly one mutant
  (the ill-formed "drop `have = …`") from killed *and* total wherever that line
  exists. v2 transfer 9/13 → 8/12, v3 10/13 → 9/12, v4 13/13 → 12/12, large
  transfer 13/13 → 12/12, withdraw 4/5 → 3/4. No other output changed; build hashes
  unchanged.

### 4. Types (`wf.TYPES = True`)
- **Change:** a minimal type checker inside the well-formedness pass, over Int,
  Account, Owner, Bool, Event and Unit, with handler signatures in `wf.SIG`:
  - `+ -` need Int; `< <= > >=` need Int; `= !=` need the same type; `and`/`or`
    need Bool;
  - `need` and `ensure` must be Bool; `keep` must be a value;
  - a body expression statement must be Unit (no discarded values);
  - a `let` cannot bind Unit; constructor arguments can't be Unit;
  - parameter types are limited to Int and Account, and return types are rejected.
- **Evidence:** `test_f7_values_do_not_leak_python_semantics` rewritten to go
  through `check_module`. With `TYPES = False` the module is accepted (verified), so
  the test discriminates. Added `test_f7_type_errors_are_rejected` (8 cases).
- **Snapshot diff vs step 3:** pytest counts only. No mutant of any example became
  ill-typed (swaps stay within a type, and `debit`↔`credit` share a signature), so no
  spec-strength number moved.

### 5. Honest statuses: vacuous ∅, unconfirmed ?, tested-only ◐
- **Change:**
  - **∅ vacuous:** `Prover.vacuity` checks whether any input passes the needs at all.
    It says whose fault it is: "no input passes its own needs" versus "… given the
    specs of X". The badge reads "Never succeeds" (red).
  - **? unconfirmed:** a refutation whose counterexample doesn't replay in the
    interpreter. It is no longer shown as ✗ "Broken" but as "Not proven:
    counterexample not reproduced", with an explanation.
  - **◐ tested-only:** a prover `unknown`, with at least 50 random successes and no
    violation (`report.TESTED_MIN`).
  - **Mutation:** only *confirmed* refutations count as prover kills.
- **Bug caught by the protocol:** my first `_status` rule used `all(proven or
  tested)` and labelled fully proven definitions "Tested only". Two F3 xfails
  XPASSed unexpectedly, which exposed it; fixed to `any(tested)`.
- **Evidence:** XPASS on `…vacuous_spec_is_flagged` and
  `…unconfirmed_refutation_is_not_shown_as_broken`. The unconfirmed exhibit moved
  from `rebalance` (which step 7 should make provable) to a callee with a spec that
  is too weak (`take` says nothing about `ledger.total`). New tests:
  - vacuity attributed to a callee (`liar`);
  - ◐ with test support versus `?` without it (`rare`, 0 successes);
  - `test_f5_tested_only_tier_exists`.

  ○ stays a strict xfail (needs runtime checks, roadmap 4).
- **Snapshot diff vs step 4 (all explained):**
  - `rebalance`: `✗ … Broken guarantees` → `? … Not proven: counterexample not
    reproduced`. Intended.
  - **`settle` 14/14 → 13/14**, with the survivor `` `to` → `house` in
    `audit.emit (Settled …)` ``. The mutant is a real bug. The prover's
    counterexample `settle A A B 0 0` doesn't replay, because it violates
    transfer's needs (`from != to`, `amount > 0`), which `settle` can't see through
    `payout`'s spec. Random tests miss it at 150 runs (0 violations) and catch it at
    500 (3). The old 14/14 counted an unreplayable refutation; 13/14 is the honest
    number. Step 8 should recover the kill.

### 6. Spec strength per layer, payload mutants, ratchet baseline
- **Change:**
  - `mutation_score` runs every layer on every well-formed mutant. The headline is
    **guarantee strength**: mutants caught by a confirmed prover refutation, by the
    mutant being vacuous (it can never succeed, though the clean original can), or
    by a random test violating a guarantee. Examples are reported separately.
  - Checker exceptions are listed, not counted as kills.
  - New mutants for failure payloads.
  - `kern/baseline.py` and `kern baseline FILE.k` record guarantee strength plus
    guarantee clauses (`<module>.strength`). `check` flags "Weaker than baseline"
    when guarantees are removed, strength falls, or a clean definition stops being
    clean.
- **Found while testing:** without counting vacuous mutants, payout's guarantees
  "missed" `transfer to to amount`, which makes payout never succeed and which the
  page would show red. Counting it is correct, because the original is clean.
- **Evidence:** XPASS on `…failure_payloads_are_mutated`,
  `…removed_guarantees_are_surfaced` (now baseline-based) and
  `…spec_strength_drops_when_guarantees_are_deleted`. Measured for v4 transfer:
  guarantees **9/13** with the spec intact versus **3/13** with every keep/ensure
  deleted. The 3 are the audit mutants `emit` still catches. The any-layer number
  is 13/13 in both cases. Expectations corrected by measurement:
  - I'd guessed 1/13 for the weakened spec; it is 3/13.
  - I'd assumed payout stays unflagged; it is correctly flagged "was clean at the
    baseline, is not now", because its proof rested on transfer's deleted ensures.
- **Snapshot diff vs step 5 (all explained).** "Total" grows by one per payload
  mutant: transfer and withdraw have `Insufficient (amount - have)`. "Guarantees" is
  the new headline:

  | Definition | Before (any layer) | Guarantees | Examples | Together |
  |---|---|---|---|---|
  | v2 transfer | 8/12 | 6/13 | 9/13 | 9/13 |
  | v3 transfer | 9/12 | 6/13 | 10/13 | 10/13 |
  | v3 payout | 12/12 | 10/12 | 12/12 | 12/12 |
  | v4 transfer | 12/12 | 9/13 | 10/13 | 13/13 |
  | v4 payout | 12/12 | 12/12 | 12/12 | 12/12 |
  | settle | 13/14 | 9/14 | 9/14 | 13/14 |
  | deposit | 3/3 | 3/3 | 2/3 | 3/3 |
  | withdraw | 3/4 | 2/5 | 4/5 | 4/5 |
  | month_end | 9/9 | 5/9 | 9/9 | 9/9 |

  v4 transfer's 4 misses are all *when it refuses*: `have` read from the wrong
  account, the need dropped, `>=` → `>`, and the payload `-` → `+`. No guarantee
  can say that yet; examples catch them. settle's prover catches only 2/14, and
  month_end's 3/9, because their callees' specs lack the inherited failure
  conditions. Step 8 targets that.

### 7. Inferred write sets: frame for modular proofs, shown to readers
- **Change:** `infer.writes` computes which balances a call can change, as parameter
  names. It follows `let` aliases and maps callee writes through call arguments; an
  account it can't name becomes "any account". In `modular_call`, only those
  indices get fresh values; before, the whole balance array was havocked, so the
  frame is now inferred, not declared (F4). Readers see it:
  - `writes from to` in iface/spec/editor views (4 tokens; `writes` is one token);
  - "Changes balances of: …" on the page;
  - an informational "No guarantee says what the new balance of x is". This does
    *not* change the status: the guarantees are still true, but the page now shows
    what they leave out.
- **Evidence:** `test_f3_guarantees_reveal_extra_writes` XPASSed. The stealing body
  now shows `writes from to x y` and "no guarantee … x, y". New tests:
  `test_f3_inferred_frame_proves_a_two_callee_definition` and
  `test_f3_write_inference_through_callees_and_lets`. The "prover reads no callee
  body" test had to keep `writes` equal too, since `writes` is part of the inferred
  interface. A garbage body writing only `from` makes payout vacuous, correctly
  attributed to transfer's spec contradicting its own frame.
- **Snapshot diff vs step 6 (all explained):**
  - **`rebalance`:** `?` → ✔ proven, guarantees 4/4. `ledger_large` exits 0 for the
    first time.
  - **`writes …` lines:** iface +3–5 tokens, spec +3–5, editor +4–10 (e.g. payout
    editor 246 → 255).
  - **settle prover 2 → 1, month_end 3 → 2:** in both, the lost kill is `fee` →
    `amount` in the callee call. The refutation is real (from' = from − 2·amount),
    but with the tighter frame Z3 picks a witness that hits transfer's hidden
    `Insufficient`, so it doesn't replay and doesn't count. In a fresh process with
    whole-ledger havoc the same mutant is `refuted`. Most of month_end's mutants are
    `unconfirmed` for that reason, which is the F1 gap step 8 targets. Caveat
    observed: within one long-running process, Z3's choice of witness can depend
    on earlier solver calls. CLI runs (one process per module) are reproducible.
  - No build hash changed.

### 8. Body-level failure conditions become inferred interface
- **Change:** `infer.lifted` states every condition a call needs in order to succeed
  in the caller's pre-state: its own body `need`s, and transitively callees' spec
  and lifted needs, substituted through arguments. A condition is lifted only if it
  comes before the first ledger write, or reads no balances at all (owners are never
  written), and never involves `clock.now`. `let`s are substituted only when bound
  in the pre-state.
  - `modular_call` assumes the callee's lifted conditions. That is sound: if the
    callee succeeded, they held in its pre-state, which is the state at the call.
  - The spec view shows the definition's **own** body-level ones as
    `body need … else …`. The proof page's "Refuses when" shows all of them,
    inherited ones labelled "(from transfer)".
- **Presentation trade-off, measured** (tokens; editor / own-plus-transitive
  source in brackets):

  | Option | payout spec | settle editor | month_end editor |
  |---|---|---|---|
  | A: own + inherited in full | 176 | 374 (0.73) | 343 (0.55) |
  | **B: own only (chosen)** | 90 | 288 (0.56) | 248 (0.40) |
  | C: own + reference per callee call | 104 | 302 (0.59) | 258 (0.41) |

  A nearly doubles caller views. C (`body need transfer from house fee`) is
  misleading, because only part of the second call's conditions is liftable. With
  B, inherited conditions are one hop away in the callee's spec view; the prover and
  the proof page always use the full set.
- **Bug found by the new page test:** `views.OPS` emitted a raw `<` into HTML
  ("balance of from < amount"), which is invalid markup and stripped by strict
  parsers. It predates this pass and is fixed (`&lt;`/`&gt;`).
- **Evidence:** the F1 test was rewritten to look for the lifted text; it fails on
  the step-7 tree, which has no `body need` lines. New tests:
  - `test_f1_lifting_stops_at_the_first_write`: the second transfer's balance check
    is not lifted, while its owner check is;
  - `test_f1_modular_proof_assumes_lifted_conditions`: `payout to to …` inside
    settle is now ∅ vacuous.
- **Snapshot diff vs step 7 (all explained):**
  - **settle:** guarantees 9/14 → **14/14** (prover 1 → 14), together 13/14 → 14/14.
    The step-5 survivor (`to` → `house` in `Settled`) is now caught.
  - **month_end:** guarantees 5/9 → **9/9** (prover 2 → 9).
  - **`body need` line, +20 tokens wherever a spec view of transfer or withdraw is
    shown:** transfer spec 111 → 131 and withdraw spec 61 → 81. Callers that embed
    those spec views grow too: payout editor 255 → 275, rebalance editor 230 → 250.
    transfer's own editor view shows its body, not the lifted line, so it is
    unchanged. (Corrected: an earlier version of this entry had it backwards.)
  - transfer's own four "when does it refuse" misses remain by design. The lifted
    line is derived from the body, so it mutates along with it and can't catch
    anything; only a written guarantee or an example can.

### 9. Bad calls are rejected at the boundary
- **Change:** `Interp.run` checks arity, Account arguments (must exist in the world)
  and Int arguments before taking the snapshot or running anything. With step 4's
  types, accounts inside a body can only come from parameters, so an unknown
  account can no longer arise mid-body. Every outcome of a valid call is `ok` or a
  declared, inferred failure.
- **Evidence:** the F4 xfail was rewritten into
  `test_f4_bad_calls_are_rejected_before_anything_runs` (4 kinds of bad call; the
  world is unchanged after each) and
  `test_f4_every_outcome_of_a_valid_call_is_ok_or_an_inferred_failure` (7
  definitions × 200 random calls).
- **Snapshot diff vs step 8:** pytest counts only.

### 10. Why store: interface-anchored, orphans, obligation lint, content hash
- **Change:**
  - **`why.anchor_hash(defs, d)`:** why entries anchor to the definition's own hash
    *plus* its inferred interface (uses, fails, writes, lifted conditions), so a
    callee change that alters a caller's interface marks the caller's why stale.
    `Def.hash` (shown as `[build]`) is unchanged and still excludes examples.
  - **`Def.content_hash`:** covers examples too, as the key for the store in roadmap
    step 2.
  - **`why.orphans`:** `check` reports entries whose definition no longer exists
    (terminal and page). `anchor` keeps them and warns instead of rewriting
    silently.
  - **Rule-3 lint:** entries containing *must / never / always / only / required /
    do not / shall* are flagged "reads like an obligation". This is a heuristic, so
    it is a warning, not an error.
  - **`kern why FILE unknown`:** a clean error (exit 2) instead of `KeyError`.
- **Re-anchoring (a confirmation, recorded here):** both `.why` files were
  re-anchored under the new scheme. `ledger.why` is anchored to v3 as before, and
  `ledger_v4.why` to v4. Entry text is unchanged; none of the three entries depends
  on the interface now being part of the anchor.
- **Evidence:** the four F6 xfails were rewritten against the new API and pass:
  - hash versus content hash;
  - v1 → v2 payout: same `hash`, different anchor, stale in v1, fresh in v2;
  - orphans: reported, kept by `anchor`, shown in the terminal;
  - obligation lint: transfer's "must go through payments.send" entry is flagged,
    and payout's isn't.

  Added `test_incidental_why_for_unknown_name_is_a_clean_error`. These tests use
  APIs that don't exist before this step, so their XPASS-first check is "the API is
  new" rather than an observed flip.
- **Snapshot diff vs step 9 (all explained):**
  - transfer's why gains the obligation warning in all 5 checks.
  - In v1, payout's why is now **STALE**: its text is unchanged, but its interface
    lacks `SameAccount`/`from != to` compared with the anchored v3. That is what the
    README originally claimed for v2, and what the audit found false; it is true now.
  - transfer's v1 stale message has the new wording.

### 11. Docs
- README: new `check` steps, tiers, baseline command, view token counts (payout v3:
  iface 40 → 45, spec 72 → 77, editor 222 → 251), and a demo loop rewritten with
  measured guarantee/example numbers.
- CLAUDE.md: file list (`wf.py`, `baseline.py`) and "Known limitations" rewritten
  for the fixed state.
- AUDIT.md: a status-after-fixes table, plus the rollback correction.

## Before → after, whole pass

**Tests:** 37 passed, 23 xfailed → **81 passed, 2 xfailed**. The two that remain are
tier ○ and record/replay (roadmap 4).

**Build hashes:** identical for every definition in every example (sorted
comparison of all `check` outputs). The AST hash never moved; only the *why anchor*
changed scheme, in step 10.

**Examples:**

| Definition | Before: status, "spec strength" | After: status, guarantees / examples / together |
|---|---|---|
| v1 transfer | Broken (2 confirmed counterexamples) | unchanged |
| v1 payout | Depends on unproven code | unchanged; why now flagged stale (interface changed) |
| v2 transfer | proven, 9/13 | proven, 6/13 / 9/13 / 9/13 |
| v3 transfer | proven, 10/13 | proven, 6/13 / 10/13 / 10/13 |
| v3 payout | proven, 12/12 | proven, 10/12 / 12/12 / 12/12 |
| v4 transfer | proven, 13/13 | proven, 9/13 / 10/13 / 13/13 |
| v4 payout | proven, 12/12 | proven, **12/12** / 12/12 / 12/12 |
| settle | proven, 14/14 | proven, **14/14** / 9/14 / 14/14 |
| deposit | proven, 3/3 | proven, 3/3 / 2/3 / 3/3 |
| withdraw | proven, 4/5 | proven, 2/5 / 4/5 / 4/5 |
| rebalance | **✗ Broken** (false) | **✔ proven**, 4/4 / 4/4 / 4/4 |
| month_end | proven, 9/9 | proven, **9/9** / 9/9 / 9/9 |

The old single number credited examples and crashes to the spec. The new headline
is lower wherever examples did the work (v3 transfer: 10/13 → guarantees 6/13), and
equal or higher where modular proofs now see through callees.

**View tokens** (v4 / ledger_large): iface +3–5 everywhere (`writes`); spec +3–5,
or +23–24 where the definition has its own body need (transfer 107 → 131, withdraw
58 → 81); editor +3–29 (payout 246 → 275).

### Final touches
- `render._how` for `!` conditional now says the callee's guarantees "are not
  proven". The old "and they currently do not [hold]" is wrong when the callee is
  merely ◐ or `?`.
- `test_f2_caller_proof_does_not_read_callee_body` now pins the full inferred
  interface a modular call reads (`uses`, `writes` *and* `lifted`), not just the
  first two.

## Clock: one time per top-level call

Decided after the fix pass (the open question since the `emit` feature). Same
protocol: a pre-change snapshot, three strict xfails that had to XPASS, and a
snapshot diff with every line explained.

- **Change:**
  - **Interpreter:** `clock.now` reads `world.clock` without advancing it.
    `Interp.run` advances the clock by one *after* each call, refused calls
    included. A bad call (rejected at the boundary or a runtime error) doesn't
    advance it, because it didn't happen.
  - **Prover:** the call's time is one symbolic value in the pre-state, shared with
    callees.
  - **Specs:** `clock.now` is a pure read, so specs may use it. `clock.now'` is
    rejected, since the time is the same before and after.
  - **Lifting:** conditions involving `clock.now` are lifted, even after writes.
  - **Mutants:** new one, `clock.now` → `0`.
  - **Example worlds:** take `now:N`, so a counterexample that depends on the time
    replays (`eg A:1@u1 B:847@u1 now:8 | …`).
  - **Random tests:** draw the time from a separate random stream, so every random
    world is unchanged.
  - **New example:** `ledger_v5.k` (v4 with `clock.now` instead of `_` in the emit
    clauses), with `ledger_v5.why` re-anchored from v4's (same text).
- **Evidence:**
  - `test_clock_spec_pins_the_timestamp`: rejected before; now proven, and a body
    emitting timestamp 0 is refuted with a replayed counterexample.
  - `test_clock_deadline_is_lifted_to_callers`: `need clock.now <= expires` was
    invisible to callers.
  - `test_clock_timestamp_mutant_survives_underscore_but_not_clock_now`.

  All three went XPASS. New tests: one value per call with ticks between calls
  (payout's two events both get 7, clock 8 after); no after-state; a
  time-dependent counterexample carries `now:N` and replays. Updated: a refused
  call now advances the clock (rollback test); totals +1 for the timestamp mutant.
- **Snapshot diff (all explained):**
  - Every body that emits `clock.now` gains one mutant, `clock.now` → `0`. It
    *survives* wherever the spec says `_` or nothing about the audit log: v2/v3/v4
    transfer (e.g. v4 9/13 → 9/14) and `ledger_large`'s transfer, settle and deposit
    (they still use `_`).
  - **v5 transfer: guarantees 10/14**, the timestamp mutant caught. v5 payout:
    12/12.
  - Random-test counts are unchanged everywhere (separate time stream). Build hashes
    are identical. Tests: 81/2 → 87/2.
- **Cost:** payout's two events share a timestamp; their order is still fixed by
  position. `clock.now` in an emit clause costs 2 tokens more than `_` (10 vs 8).

## `ok otherwise`: a spec can say when a call succeeds

This closes F3's remaining gap ("a guarantee can't say *when* a call succeeds"). It
is opt-in, and a definition that opts in restates every refusal itself; nothing is
inherited. Same protocol: a pre-change snapshot (tests, check output for v3/v4/v5,
build hashes), the change, then a diff.

- **Change:**
  - **Syntax:** a spec line `ok otherwise` (4 tokens with indent and newline; `else ok`,
    `only these` and `succeeds otherwise` cost the same). It means: if every spec
    `need` holds, the call succeeds. Spec needs already run as guards, so a
    refusal is written once, in the spec, and enforced by that line.
  - **Prover:** records every refusal point after the spec needs: body `need`s,
    and each callee's spec needs at the call. The goal is that none of them can
    fail on a path where the spec needs held. It has its own premise, not the
    success path.
  - **Callees:** a callee that can refuse beyond its spec (a body `need`, or a
    callee that refuses) and has no `ok otherwise` makes the claim *unknown*:
    "relies on transfer, whose spec does not say when it refuses". This mirrors
    `emit`.
  - **Counterexamples:** confirmed only when the interpreter's replay refuses
    with every spec need true. Random tests report the same thing as a violation.
  - **Proof page:** "succeeds whenever the needs above hold: it refuses for no
    other reason". When the claim is proven, the lifted "(from X)" and
    "passed through" rows are dropped from "Refuses when"; when it isn't, they
    stay.
  - **Hash:** includes the marker only when present. Unmarked definitions keep
    their hashes.
- **New examples:**
  - `ledger_v6.k`: transfer moves `Insufficient` into its spec and adds
    `ok otherwise`. Payout restates all its refusals but keeps `fee >= 0`.
  - `ledger_v7.k`: payout says `fee > 0`.
  - `.why` files copied from v5 and re-anchored (same text).
- **Evidence:**
  - **v6 payout ✗**, `eg A:1@u1 B:253@u1 | payout A B B 1 0 -> BadAmount`, replayed:
    transfer refuses a zero fee, which payout's spec allowed. In v5 the same bug
    gives a clean page. Random tests find it too (2 violations).
  - **v7:** both claims are proven. Payout's `Insufficient (amount + fee - balance
    from)` holds only through transfer's `ensure` (the second call needs
    `balance' from >= fee` after the first debited `amount`).
  - **Unmarked definitions are unchanged:** `check` output for v3, v4 and v5 is
    byte-identical before and after, and so are all build hashes.
- **Mutation numbers, read carefully:**
  - **v7 transfer: guarantees 10/10** (v5: 10/14). The four v5 survivors are *not*
    caught. They're gone: the `need` they mutated left the body. Mutants are
    body-only, so anything moved into the spec drops out of the spec-strength
    measurement. That's a measurement gap, not new strength.
  - **What the obligation itself catches**, measured by keeping a redundant body
    need: 12/14. `>=` → `>` and `from` → `to` are killed by `ok otherwise`. The
    two survivors (drop the need, flip its payload) are equivalent mutants,
    because the spec guard runs first.
  - **v7 payout:** 12/12, as in v5.
- **Default on or not, measured:** `ok otherwise` added to every definition of v5
  and `ledger_large`, nothing else changed. **1 of 9 passes** (`deposit`). The
  other eight are ✗ with replayed counterexamples, e.g.
  `rebalance A B 0 -> BadAmount` and `month_end A B B 0 1 -> FeeTooLarge`. Hence
  opt-in.
- **Cost (tokens):**

  | | v5 | v7 |
  |---|---|---|
  | transfer, full definition | 233 | 235 |
  | transfer, spec view | 133 | 135 |
  | payout, spec view | 97 | 163 |
  | payout, editor view | 281 | 349 |

  Restating is what costs: payout's spec grows by 66 tokens, and each level of a
  call chain (settle, month_end) would repeat the same conditions.
- **Not done:**
  - Inheriting callee refusals. This needs a readable pre-state form for
    conditions after a write: payout's second-call condition depends on
    transfer's effect and on aliasing between `to` and `house`.
  - Mutating spec lines.
- **Tidying:** the `ok` claim is never promoted to ◐ tested-only, because random
  successes can't test a claim about refusals. Refusal rows negate `and`/`or` by
  De Morgan ("amount ≤ 0 or fee ≤ 0", not "not (amount > 0 and fee > 0)").
- **Tests:** 87/2 → 97/2 (10 new, `test_f3_ok_otherwise_*`).

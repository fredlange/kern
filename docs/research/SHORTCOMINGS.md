# Shortcomings: one register

Date: 2026-11-05. Code state: `0417495`.

This is the one list of what Kern doesn't do well yet, or hasn't shown. The README's "Where it
falls short" picks the most important entries from here. CLAUDE.md's "Known limitations" says the
same things in more detail; pointing it here is a follow-up. The evidence stays in the note that
measured it (CHALLENGE, COMPARE, ROADMAP_RUN, AUDIT, FIXES, C2's PROTOCOL). This file has one number
and a link for each entry, not a third copy of the curves.

## How to use it

Every entry has a **kind**, and the kind decides what "fixed" means:

| Kind | Fixed when | Never |
|---|---|---|
| **missed target**: a pre-registered threshold failed | a *new* run of the frozen protocol passes; the old run stays a fail (CHALLENGE.md, rule 1) | retune a frozen threshold |
| **untested**: a claim no experiment has tested | the experiment runs as pre-registered, pass or fail | call it fixed by argument |
| **code gap**: the code lacks something or gets it wrong | its gap test in `kern/tests/shortcomings.rs` starts failing, and becomes an ordinary test in the same commit | close it without a test |
| **trade-off**: a measured cost we may decide to accept | Fredrik decides to accept it, or a change measurably reduces it | hide the number |

When an entry closes, move it to "Closed" with the commit and the evidence. Don't delete it.

**The gap tests** (`kern/tests/shortcomings.rs`) each assert the behaviour we want, with
`#[should_panic(expected = "gap Sn …")]`, so an unrelated crash doesn't count as the gap. Some features
have no syntax yet (records, text, `match`, named constants, `sum`, `clock.tick`, events in `eg`).
Their tests use a *placeholder* syntax. Whoever builds the feature must rewrite the test to the real
syntax; otherwise it keeps panicking for the wrong reason. The other tests flip on their own.
`every_test_and_path_the_register_names_exists` fails if this file names a test or a
`kern/tests/` path that doesn't exist.

## Open

### S1. Working through views costs more tokens than TypeScript (C2)
- **Kind:** missed target. **Weakens:** bet 1 (reading less is cheaper).
- **Evidence:** K-view used 1.03 / 1.19 / 1.33× TS's tokens, against a frozen ≤ 0.7×
  (0.97–1.06× with cache reads; [PROTOCOL.md, C2 results](../../experiments/challenge/c2/PROTOCOL.md)).
  The note added on 2026-11-05 at the end of PROTOCOL.md, computed by `view_cost.py`, finds two causes:
  - **A fixed cost, on the first request:** cache_creation was about 3.9–4.0k for K-view against
    1.1–1.3k for TS. That is REFERENCE.md (2,126 proxy tokens) plus a longer preamble. Without the
    first request, K-view is 0.78 / 0.97 / 1.11× TS.
  - **The views themselves, on T3:** the six editor views read 3,848 tokens, more than the whole
    10-definition module (2,267). All 35 view calls asked for `editor`; none used the cheaper
    `spec`. The editor view repeats each callee's spec, and related definitions share callees.
  - **The MCP tool schemas cost nothing under the frozen metric.** They sit in the cached prefix:
    the first-request cache_read is the same in every run of an arm. The original write-up blamed
    them; that doesn't hold.
- **Fixed when:** a new C2 run, with the same 0.7× threshold, passes.
- **Simulated, 2026-11-05** (PROTOCOL.md, second dated note; `view_cost.py`, section 6). These are
  other view designs replayed on the recorded runs' own view calls, with agent behaviour held fixed,
  so they're optimistic:
  - Combining two changes cuts view tokens by 40–66%: don't repeat a callee spec already shown, and
    show `spec` everywhere except the definition being edited. K-view would then be 0.84 / 1.06 /
    1.04× TS.
  - **No view design reaches 0.7× on this module.** On T2 and T3, everything after the first
    request already costs more than 0.7× TS, with no REFERENCE at all. On T1, REFERENCE.md would also
    have to shrink from 2,126 to about 770 proxy tokens.
  - So the frozen threshold can't be met on a 10-definition module through reading or the reference
    alone. Whether views win at all is a question about larger programs.
- **Next:**
  - (b) is now the main question: a pre-registered C2b on a larger module (`escrow` s3 or
    `ledger-large`), with the two view changes built first. It needs Fredrik's approval (agent runs).
  - (a), the two view changes, is worth building anyway: it's 40–66% fewer view tokens.
  - (c) REFERENCE.md's size is the language tax. Cutting it is Fredrik's call, since C2 froze it.
  - Not worth it: dropping the four tools the runs never called (`kern_why`, `kern_run`,
    `kern_replace`, `kern_remove`) saves about 330 proxy tokens, and 0 under the frozen metric.

### S3. A wrong spec, proven, is still wrong
- **Kind:** untested. **Weakens:** bet 2 (a proven spec means correct code) and bet 3 (a human can
  judge the guarantees).
- **Evidence:** C1's B10 drops `part <= amount` from refund's spec. Kern, TypeScript and Go all
  missed it. After S1 the ratchet makes it *visible as a spec change*, which a human must judge,
  but it is not refuted ([CHALLENGE.md, C1 addendum](CHALLENGE.md#c1-seeded-bugs-run-2026-11-03)).
  C3 (spec gaming) is predicted to **fail**:
  - re-running `kern baseline`, or editing the `.strength` file, resets the ratchet unseen;
  - a dodging `need` gets through where no `ok otherwise` caller or `eg` covers it.

  `check --against REV` (S1) compares with a git revision, which partly covers the first route.
- **Fixed when:** C3 and C4 run as pre-registered. C3's proposed fix is a guarantee diff against
  the last committed baseline, on the proof page. C4 is the blind test of whether Fredrik spots 5
  of 6 wrong specs on proof pages.
- **Next:** run C3 (18 agent runs). C4 needs 45 minutes of Fredrik's time.

### S4. One author, one model family
- **Kind:** untested. **Weakens:** every measured result.
- **Evidence:**
  - The same agent wrote Kern, the TypeScript and Go versions, the specs and the C1 bug catalogue,
    in a ledger domain that suits Kern ([COMPARE.md, caveats](COMPARE.md#caveats)).
  - C2's "cold" authors were the same model family that designed Kern, with the reference in
    their prompt.
  - C2 had N = 3 per cell, and every cell was 3/3, so the arms aren't distinguishable on success.
  - T3's task text names its trap.
  - 18 runs were replaced for harness faults, and the chained-Bash rule was narrowed, both
    accepted by Fredrik on 2026-11-04 (PROTOCOL.md).
  - C1's blind bugs were only 4 body bugs.
- **Fixed when:** C5 (a second domain, a fresh author) runs as pre-registered, and C2 is rerun
  with another model family and with TypeScript and Go versions we didn't write.
- **Next:** C5. It also produces the ranked list of language gaps that S5 needs.

### S5. A small language
- **Kind:** code gap. **Weakens:** bet 1 at scale, and every result outside the ledger domain.
- **Evidence:** CLAUDE.md, "Known limitations"; ROADMAP_RUN.md, P (the gap table for ordinary
  programs). The ledger, clock and audit handlers are built into the interpreter.
  examples/random-portal shows joke ids, not jokes, because there is no text.
- **Fixed when**, per feature (P = placeholder syntax):

  | Missing | Gap test |
  |---|---|
  | records (P) | `s5_records_check` |
  | text (P) | `s5_text_checks` |
  | `match` (P) | `s5_match_checks` |
  | recursion (rejected: no termination measures) | `s5_recursion_is_accepted` |
  | nested loops | `s5_nested_loops_check` |
  | variable × variable | `s5_variable_times_variable_checks` |
  | named constants (P) | `s5_named_constants_check` |
  | imports across packages (P: the `kern.pkg` dependency line) | `s5_import_across_packages_loads` |
  | effects that return values, Accounts as state values, iteration over accounts | not written yet |
- **Next:** records, then `match`, then text (roadmap step 1), ranked by C5's gap list once it
  exists.

### S6. Loops: the frame can't be stated, and there is no sum
- **Kind:** code gap. **Weakens:** spec strength for loops.
- **Evidence:** the loop frame is inferred and can't be stated. So in batch.k, a mutant that moves a
  loop bound by one (`hi + 1`, `lo - 1`) survives the guarantees. Events inside a loop are flagged,
  not specified ([ROADMAP_RUN.md, S4 results](ROADMAP_RUN.md#s4-results)).
- **Fixed when:** `s6_a_guarantee_kills_the_sweep_one_too_many_mutant` (on `advance_subs`; closing it
  means stating the frame in batch.k itself) and `s6_a_sum_invariant_proves` (P).

### S8. Mutants are body-only
- **Kind:** code gap. **Weakens:** spec strength as a measure.
- **Evidence:** moving a `need` from the body into the spec removes its mutants. In the ledger tour,
  transfer goes from 14 mutants to 10, so v6–v7's 10/10 is a smaller denominator, not more
  strength (examples/ledger-tour/README.md; [FIXES.md](FIXES.md)).
- **Fixed when:** `s8_spec_need_mutants_are_generated`.
- **Note:** a mutated spec that is still self-consistent is S3's problem, not this one's. Spec
  mutants can only be caught by examples, callers' `ok otherwise` or the ratchet.

### S9. The `why` obligation lint is a keyword heuristic
- **Kind:** code gap (a measurement, not a test). **Weakens:** F6 (nothing that must be obeyed lives in
  prose).
- **Evidence:** it flags modal words ("must …"). A rule phrased without them passes, and a
  rationale that uses them is flagged ([AUDIT.md](AUDIT.md), F6).
- **Fixed when:** a better lint is measured on a labelled set of why entries, against the keyword
  lint's score.

### S10. Audit events in examples, and time inside a call
- **Kind:** code gap.
- **Evidence:** CLAUDE.md, "Known limitations".
- **Fixed when:**
  - `eg` can assert events: `s10_eg_can_assert_events` (P);
  - one call can see two instants (`clock.now` is one value per top-level call):
    `s10_one_call_can_see_two_instants` (P).
- **Not a gap:** "emits nothing" can be said today, as `emit Never when 1 = 0`. That is an exact empty
  sequence, pinned by the ordinary test
  `s10_emits_nothing_is_expressible_today_as_a_false_conditional_emit`. A readable form would be
  ergonomics.
- **Needs a decision first:** `emit` is exact-sequence only. There's no agreed alternative ("contains",
  "at least"), so there's no test.

### S11. Tier ○ (runtime-checked only) exists only in the host
- **Kind:** code gap. **Weakens:** F5 (every claim carries its strength).
- **Evidence:** the checker's proof page has no ○. The host checks invariants at run time.
- **Fixed when:** `f5_runtime_only_tier_exists` (`kern/tests/fundamentals_back.rs:628`).

### S12. The host: a reply that never arrives leaves the request pending
- **Kind:** code gap.
- **Evidence:** HttpNet's 2 s transport timeout already becomes the `fail` call. What's missing is
  the case where the reply never reaches the host (a dropped reply, a crash after flush), or the
  module refuses it. Then the request stays pending forever ([HOST.md](../HOST.md)). Output delivery
  is at least once.
- **Fixed when:** `s12_a_request_with_no_reply_times_out_into_a_call` (examples/limits, a Net that
  never answers). At-least-once delivery may be the contract to keep, with receivers
  deduplicating. That is a decision, not a test.

### S13. At a leaf, Kern doesn't win
- **Kind:** trade-off.
- **Evidence:** transfer's editor view is 234 tokens; the TypeScript function alone is about 170.
  Full Kern source for a definition and its callees costs more than TypeScript (purchase 1,258
  against 963). The saving comes only from reading callees as specs
  ([COMPARE.md](COMPARE.md#findings), findings 4–5).
- **Decision needed:** accept it as the price of a checked interface, or drop the inferred lines
  when there's nothing to skip (AUDIT F9 suggested this; the gain is small).

### S14. `ok otherwise` makes callers restate every refusal
- **Kind:** trade-off.
- **Evidence:** payout's spec view grows from 97 to 163 tokens. Nothing is inherited, every refusing
  callee must opt in too, and only 1 of 9 existing definitions passed with it on by default
  ([FIXES.md](FIXES.md)).
- **Measured, 2026-11-05: `need as` doesn't cover it.** On ledger_v7's payout (spec view 160 tokens
  as written):
  - `need as transfer from to amount` plus `need as transfer from house fee` is **refuted** (✗
    `ok otherwise`; `payout A B B 1 1 -> Insufficient 1`). The second call runs after the first
    one wrote, and `need as` is only right for a callee that runs first.
  - `need as` for the first call, plus restating what the second adds, proves, but its view is
    191 tokens. `need as` expands at parse time, so it saves the author typing, not the reader
    tokens.
- **Next:** any saving for readers needs a new form, one that shows an inherited refusal in a
  callee's terms in the view, without expanding it, and that the prover can check after earlier
  writes. That's a language proposal for Fredrik. S7's lifting past writes is the machinery it
  would need.

### S15. Random tests: 150 runs per mutant
- **Kind:** trade-off.
- **Evidence:** the `settle` audit mutant needed 500 random runs to be caught; the prover now
  catches it. This interacts with S16: more runs make strength more accurate and `check` slower.

### S16. Paths fork at every `if`, and mutation testing is most of a check
- **Kind:** code gap (a measurement, not a test). **Weakens:** bet 1 (`check` stays in the inner loop).
- **Evidence:** on Rust, each `if` still costs ×2.2 per definition once k ≥ 4. At d=3: k=4 takes
  0.59 s, k=5 1.31 s, k=6 2.95 s. Mutation testing is 90–95% of a full check (P1 0.89 s with `--quick`
  against 8.83 s full). C6's P1 sits at k=4, so it passes with a 9–15% margin
  ([CHALLENGE.md, C6 addendum 2026-11-05](CHALLENGE.md#c6-prover-stress-run-2026-11-03)).
- **Fixed when:** the k-sweep of `cargo run -p kern-validation --release --bin c6` stays under 1 s per
  definition at k=6. Fredrik sets the bar; this one is a proposal.
- **Done, 2026-11-05: mutants are scored in parallel** (`kern/src/check.rs`, `KERN_JOBS`, default 4).
  Every output is identical: both digests are unchanged, serial and parallel alike, and the goldens
  pass. On C6, with serial and parallel runs alternated and the best of three taken, P1 goes from 0.864
  to 0.365 s per definition (×2.4), P2 from 0.173 to 0.077 s (×2.2), and k=6 from 2.93 to 1.30 s
  (×2.2). Load from other sessions was 4–11 on 18 cores during the runs.
- **The next limit is Z3's:** more than 4 threads is *slower* (18 threads: 0.44 s on P1). Z3 5.3.3
  takes one global mutex (`gparams::get_module`) every time a rewriter is built, which happens in
  every solver's construction. `sample` shows most threads waiting there.
- **Measured:** a proof builds about 47 solvers on P1 and about 190 at k=6, roughly one per path per
  goal (P1: 18,294 solvers for 386 proofs). Serially, building them is about 12% of the samples and
  solving (`Z3_solver_check`) about 43%. The 12% is the part that takes the lock.
- **Next:**
  1. Build fewer solvers per proof, e.g. one solver per definition with push/pop. Z3's incremental
     mode may pick different models, so counterexample eg lines, and the digests, could change.
     That needs a check that verdicts stay the same.
  2. Score mutants in separate processes, which don't share Z3's lock.
  3. Merge paths at `if` (one path with conditional values): the curve, not the constant.
  4. Skipping random tests on prover-killed mutants would change the page's per-layer counts. That
     makes it a decision, not an optimisation.

### S17. Lifting still stops after a callee that writes
- **Kind:** code gap.
- **Evidence:** a caller that does `transfer a b x` and then `need …` still hides that need from its
  own callers. The callee's after-state is havoc plus its ensures, not an exact write, so S7's write
  log can't absorb it. This is probably the common case in real code. Loops with effects and `if`s
  whose branches wrote also still stop it (`s7_lifting_still_stops_where_the_after_state_is_not_exact`
  pins all three).
- **Next:** a design: use a callee's ensures as equations where they pin a value (transfer's
  `ensure ledger.balance' from = ledger.balance from - amount` does). That needs a proposal first.

### S18. The page shows a body need's source text even when it reads post-write state
- **Kind:** code gap (page wording; needs a decision).
- **Evidence:** the proof page's own-refusal rows print a body need's *source* text. For a need after a
  write, "refuses TooRich when balance of to ≥ 1000" means the balance *after* the debit, and the row
  doesn't say so. The spec view, since S7, shows the pre-state form. Separately, with real aliasing
  that form is a long case split (`to = from and ledger.balance to - amount < 1000 or …`), and it isn't
  simplified (`ledger.balance a - 1 + 1`).
- **Decision needed (Fredrik):** render those rows from the lifted pre-state form, or annotate
  "(after the writes above)". A readable conditional form (`if C then A else B`) would be new syntax,
  and so a proposal.

## Closed

### S7. Refusal conditions were lifted only up to the first ledger write
- **Kind:** code gap. **Closed** 2026-11-05 (the commit that adds `validation/tests/lifted.rs`; by kern-44).
- **Was:** a body `need` after a write was missing from the definition's own spec view.
- **Now:** direct writes (`ledger.debit`/`credit`, `x.set`) are logged, and a later need is rewritten
  into pre-state terms. A read that may alias a written key splits into cases (`to = from and … or to
  != from and …`), pruned by `!=` refusal points already passed. Past 8 cases, the need isn't lifted.
- **Evidence:**
  - `s7_a_refusal_after_a_write_shows_in_the_spec_view`, plus
    `s7_a_condition_after_a_write_is_rewritten_into_the_pre_state` and
    `s7_lifting_still_stops_where_the_after_state_is_not_exact`.
  - **The fuzzer doesn't cover this:** its generators never put a body need after a direct write.
    `lifted_conditions_past_writes_agree_with_the_interpreter` (validation/tests/lifted.rs) fills the
    gap with 160 seeded programs whose callers rely on the lifted condition. It finds 0 contradictions
    against the interpreter. 105 caller claims are proven, and all 105 come from the new lifting:
    against the old infer.rs, 0 are proven.
  - The fuzzer at seeds 1–3 has 0 contradictions, and both digests are unchanged. One golden moved:
    tests/probes/runtime.k `late` gains its lifted need.
- **What remains:** S17, S18.

### S2. A cold full `check` was too slow for deep call trees (C6)
- **Kind:** missed target. **Closed** by the rerun in `d959c0f` (2026-11-05).
- **Evidence:** the 2026-11-03 Python run failed both thresholds (P1 16.46 s per definition against
  ≤ 1 s; P2 110 s against ≤ 60 s). The 2026-11-05 rerun of the frozen plan on the Rust toolchain passes
  both: P1 0.85–0.91 s, P2 34.0–39.3 s, over 7 runs. The work was the same: the generator is
  byte-identical and the mutant counts are equal
  ([CHALLENGE.md, C6 addendum 2026-11-05](CHALLENGE.md#c6-prover-stress-run-2026-11-03)).
- **What remains** is S16: P1 passes by about one `if`.

For gaps found and closed before this register
existed, see AUDIT.md's "Status after the fix pass" and ROADMAP_RUN.md's FZ findings.

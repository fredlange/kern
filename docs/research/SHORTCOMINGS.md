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
have no syntax yet ( `clock.tick`, events in `eg`).
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
    - **Drafted 2026-11-05, awaiting Fredrik's approval:**
      [c2b/PROTOCOL.md](../../experiments/challenge/c2b/PROTOCOL.md). It covers escrow s3, C2's three tasks,
      N = 5 and ~$17. Its prediction (`predict.py`, at df332d5) puts K-view at 0.64–0.83× TS, so 0.7× fails
      on T3.
  - (a), the two view changes: **built 2026-11-05** (kern-9b; `kern/src/mcp.rs`, `Session`).
    - **Dedup:** the server stubs a callee spec block it has already shown, as
      `move: spec shown earlier in this session; proof ok`.
    - **Keyed on the block's text, not the hash:** a change in a callee's own callee alters the block
      without changing the hash.
    - **An earlier editor view of X counts as having shown X's spec**, but only while the lines are
      exactly equal.
    - **The `kern_view` description** now says to read with `spec`.
    - **Measured:** the recorded view calls replayed through the server give 1,584 / 1,896 / 2,584
      view tokens with dedup, against the simulation's 1,573 / 1,882 / 2,545. With the spec-first
      description followed too, 732 / 1,566 / 1,704. `kern view` and the goldens are unchanged.
    - **The test any dedup design must pass:**
      `an_editor_view_covers_its_spec_only_while_its_lines_still_hold`. A first version used line
      containment, and it stubbed a block after an edit had *removed* a line from it.
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
  examples/random-portal showed joke ids, not jokes, because there was no text (it shows jokes since `e87651d`).
- **Fixed when**, per feature (P = placeholder syntax). The last four rows joined the milestone on 2026-11-05
  (.party/DECISIONS.md, planner 22:45, on Fredrik's go-ahead); each needs its gap test written first:

  | Missing | Gap test |
  |---|---|
  | ~~records~~ first cut closed `b18367a` (Closed: "S5, records") | `s5_records_check` |
  | ~~text~~ closed `135dfca` (Closed: "S5, text") | `s5_text_checks` |
  | ~~`match`~~ closed `a8cee99` (Closed: "S5, match") | `s5_match_checks` |
  | ~~recursion~~ closed `3700f8d` (Closed: "S5, recursion with a termination measure") | `s5_recursion_is_accepted` |
  | ~~nested loops~~ closed `59c9e66` (Closed: "S5, nested loops and variable × variable") | `s5_nested_loops_check` |
  | ~~variable × variable~~ closed `59c9e66` (same entry) | `s5_variable_times_variable_checks` |
  | ~~named constants~~ closed `0acdb54` (Closed: "S5, named constants") | `s5_named_constants_check` |
  | ~~imports across packages~~ closed `b71848b` (Closed: "S5, imports across packages") | `s5_import_across_packages_loads` |
  | effects that return values | not written yet |
  | records as state values, `-> Record`, nested records, record equality (added 2026-11-05) | not written yet |
  | Accounts as state values, iteration over accounts | not written yet |
  | decimals: exact fixed-point, not binary floats (added 2026-11-05) | not written yet |
  | lists: a sequence type (added 2026-11-05) | not written yet |
- **Next:** records, then `match`, then text (roadmap step 1), ranked by C5's gap list once it
  exists.

### S9. The `why` obligation lint is a keyword heuristic
- **Kind:** code gap (a measurement, not a test). **Weakens:** F6 (nothing that must be obeyed lives in
  prose).
- **Evidence:** it flags modal words ("must …"). A rule phrased without them passes, and a
  rationale that uses them is flagged ([AUDIT.md](AUDIT.md), F6).
- **Fixed when:** a better lint is measured on a labelled set of why entries, against the keyword
  lint's score.

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

### S19. Lifting stops at callees whose ensures don't pin what they write, and at loops
- **Kind:** code gap.
- **Evidence:** after S17, lifting continues past a writing callee only when its ensures pin every
  written key exactly (`ensure R' = E`, with no `when` or `for`, or `keep R`). It still stops at:
  - a callee with a `>=`, `when`-guarded or `for`-quantified ensure on something it writes;
  - a loop with effects;
  - an `if` whose branches wrote.

  The total stays exact only with `keep ledger.total`. All of these are pinned by
  `s17_lifting_stops_after_a_callee_that_does_not_pin_what_it_writes` and
  `s7_lifting_still_stops_where_the_after_state_is_not_exact`.
- **Next:** none planned. Each needs a merge of after-states, which is the same machinery as S16's
  path merging.

## Closed

Decisions marked "under delegation" were made on 2026-11-05, when Fredrik handed them over: "you are free
to make decisions as long as they follow the fundamentals". Each one names the fundamental that decided it.

### S5, recursion with a termination measure (one row of S5)
- **Kind:** code gap. **Closed** 2026-11-05 in `3700f8d`, under delegation (.party/DECISIONS.md, 2026-11-05, lang,
  recursion); review fix `6eb422f`; compiled `606ed1c` and the host's stated depth limit `f03cb4e` (runtime).
- **Was:** recursion was rejected, because there were no termination measures.
- **Decided by** F2 (calls stay modular, a self-call too) and F5 (partial correctness must not read as total):
  a spec line `decreases E` (3 tokens) over parameters, constants and pure functions; every definition of a
  cycle needs one. Each recursive call gets a `measure` goal, `0 <= E(args) < E(params)` on the path to the call.
  In test, a self-recursive definition with a ✗ or `?` measure still showed its ensures ✔ (`result = loop_self n`
  never returns); since `6eb422f` its other goals are `!`, "relies on termination measure".
- **Evidence:** `s5_recursion_is_accepted` passes as an ordinary test; `s5_a_measure_is_checked_at_every_recursive_call`:
  a recursive and a mutually recursive pair proven, `spin n` ✗ with a replaying counterexample, `loop_self` `!`.
  Tester: valid measures ✔, bad ones `!` for self, mutual and caller cases; compiled recursion 0 divergences.
  Digests at their recorded values; fuzz seeds 1–3 0 contradictions. examples/apps/shop's `parcels` proves its
  measure and a closed form by induction.
- **What remains:** calls nest at most 200 deep at run time (interpreter, host, compiled program); deeper is a
  fault, stated on the host page. A proven measure doesn't promise a depth under 200.

### S10. Audit events in examples, and time inside a call
- **Kind:** code gap. **Closed** 2026-11-05 in `e1e1a68` (lang), under delegation (.party/DECISIONS.md, 2026-11-05,
  lang, S10); compiled `e0f04bf` and host `ea7bf8b` (runtime).
- **Was:** `eg` couldn't assert events, and `clock.now` was one value per top-level call, with no way to see two
  instants in one call.
- **Decided by** F8 (an `emit` an example pins is a mutant an example can catch): an optional fourth `|` section
  on an `eg` line, the exact sequence appended to `audit` and to each channel named (6 tokens on a 24-token line);
  an empty section means nothing appended. `clock.tick` moves time inside a call; a spec's `clock.now` stays the
  call's start time, and time never goes back across calls (compiled and host).
- **Evidence:** `s10_eg_can_assert_events` and `s10_one_call_can_see_two_instants` pass as ordinary tests. Tester:
  an `emit` after a tick that claims the start time is ✗; exact tick counts ✔; after a ticking callee or loop a
  true claim is `?` and false ones ✗ (honest, not ✔); a wrong event list fails with "appending …"; quoted
  `"a, b"` and `"a | b"` survive; time is monotone across calls in compiled program and host, refused ticks
  are undone; difftest 0 divergences; replay matches.
- **What remains:** `emit` is still exact-sequence only (no "contains"/"at least"; a decision, no test). "Emits
  nothing" in a spec is still `emit Never when 1 = 0`.

### S5, imports across packages (one row of S5)
- **Kind:** code gap. **Closed** 2026-11-05 in `b71848b`, under delegation (.party/DECISIONS.md, 2026-11-05, lang,
  imports across packages); `package.rs` fix `dbd0033`.
- **Was:** imports reached within one package only, so every example app copied its ledger.
- **Decided by** F4/F7 (one checked module): `kern.pkg` holds `use ../money` lines (3 tokens each, 0 in Kern
  source); a used package's modules are `money.<module>`; everything merges into one checked Module.
- **Evidence:** `s5_import_across_packages_loads`, rewritten to the real syntax, passes as an ordinary test. shop,
  library and lottery now `use ../money` instead of copying the ledger; their results and proof pages are
  unchanged (goldens: one line each, the package list, plus examples/money recorded). Tester: missing, cyclic,
  clashing, stray-line and transitive cases are all right.
- **What remains:**
  - An MCP edit in a used package doesn't re-check the packages that use it (F2: an agent's edit can leave a
    caller's verdict stale). A lang task.
  - Minor (tester): `use` twice is accepted silently though the decision says each name once; a non-sibling
    path (`../../x`) loads in the CLI; an absolute path says "not a package" even with a kern.pkg.

### S5, match (one row of S5)
- **Kind:** code gap. **Closed** 2026-11-05 in `a8cee99`, under delegation (.party/DECISIONS.md, 2026-11-05, lang,
  match). Generator `68f7e1f` (tester).
- **Was:** a choice between several values was an `if`/`else` chain.
- **Decided by** F4/F7 (one meaning): `match E` with arms `PAT -> STMT` (an integer, a constant, a Text literal
  or `_`), desugared at parse time into the `if` chain, so every path sees ordinary code. n arms give n paths,
  not 2^n (S16). The three-arm example is 27 tokens against 32 for the chain.
- **Evidence:** `s5_match_checks`, its placeholder the chosen syntax, passes as an ordinary test. Tester: match
  gives the same verdicts as its `if` chain on 300 generated programs; compiled `match` 0 divergences; fuzz
  seeds 1–3 0 contradictions. examples/apps/shop uses it (`advance`, all proven).
- **What remains (decided):** `match` as an expression, matching refusal outcomes, ranges or several values per
  arm. Mutant and counterexample lines show the `if` chain, not the `match`.

### S5, nested loops and variable × variable (two rows of S5; app's request #3)
- **Kind:** code gap. **Closed** 2026-11-05 in `59c9e66`, under delegation (.party/DECISIONS.md, 2026-11-05, lang,
  nested loops and v×v). Generator `68f7e1f` (tester).
- **Was:** `nested loops are not supported yet`, and `*` needed a literal on one side, so a grid key
  `row * width + col` couldn't be written.
- **Decided by** F5 (no claim stronger than the check): `*` between variables goes to Z3 as a nonlinear term;
  an unknown answer is `?`/◐, never ✔. Nested loops need no new syntax: each loop keeps its own invariants, and
  the outer loop's inferred frame havocs whatever the inner one writes unless keyed by the outer variable
  (sound, coarse; F4). 0 tokens. Linear code is unchanged.
- **Evidence:** `s5_nested_loops_check` and `s5_variable_times_variable_checks` pass as ordinary tests;
  `s5_nonlinear_verdicts_agree_with_the_interpreter`: 22 seeded claims, 14 ✔, 8 ✗ replayed, 0 undecided,
  0 contradictions. Tester: generators for both, seeds 1–3 0 contradictions, a planted frame bug caught (3,827);
  compiled nested loops 0 divergences. Digests unchanged. examples/apps/booking uses both (`clear_all`, `seat`).
- **What remains:** the coarse outer frame means the prover pins few nested-loop mutants: booking's `clear_all`
  is 10/22, the prover 1 (app's request #5). A computed key leaves a module invariant ○, as any computed key does.

### S5, text (one row of S5)
- **Kind:** code gap. **Closed** 2026-11-05 in `135dfca`, under delegation (.party/DECISIONS.md, 2026-11-05, lang,
  Text; app's request #2). Host `d2b67e7` and compiled path `e7df085` (runtime); tests `99c302b`, `2ba095e`.
- **Was:** parameters and state held only Int and Account; an app's tasks had no titles, the random portal showed
  joke ids.
- **Decided by** F2/F5 (exact, no approximation): `t:Text`, `"…"` literals (escapes `\"`, `\\`), `length t`,
  `a + b` joins, `=`/`!=`; `state NAME KEY -> Text` (default `""`), Text record fields, Text in `eg` lines. The
  prover uses Z3 strings by code point, and counterexamples quote Text and replay. Ordering on Text and Text
  refusal payloads are rejected.
- **Evidence:** `s5_text_checks`, rewritten to the real syntax, passes as an ordinary test, with proofs of joins,
  lengths, a quantified loop over a text map and a console event. Seeded floor: 84 goals, 65 proven, 19 refuted
  and replayed, 0 undecided. Tester: prover and interpreter agree on escapes, é/😀 and code points past U+2FFFF
  (gen_text, seeds 1–3, 1,027/1,027, 0 contradictions; a reversed join and `length + 1` planted and caught);
  compiled and host Text: tests/probes/texts.k 10,007 calls and fuzzdiff --texts 927 programs, 0 divergences, a
  byte-length plant caught. Digests and goldens unchanged. REFERENCE.md 2,524 → 2,627 tokens.
- **What remains:**
  - ~~A counterexample world drops Text map entries (`?` instead of ✗)~~ fixed in `7d011af` (tests `03f36a3`), with
    the same fault for record programs (an entry at a key computed from a field): a counterexample that doesn't
    replay is re-read at likely keys and kept only if that one replays. Loop digest e90c41b6af1ac4d6 →
    5633e03d4adf29df (dump diff: one goal, `!` → ✗ with a replaying counterexample).
  - Under `KERN_Z3_FRESH=ctx`/`off` one text-loop mutant is killed by random tests, not the prover (a Z3 string
    quantifier near its step limit); the default is deterministic.
  - ~~examples/random-portal still shows joke ids~~ it stores and shows joke text since `e87651d` (7/7 clean; tester:
    difftest 0 divergences, host against a local stub stores quotes, é, 😀 and newlines, replay matches). Its Docker
    `smoke.sh` hasn't been re-run since (Docker Hub timed out).

### S22. A full `check` of a loop writing a Text map and an Int map at a computed key didn't finish
- **Kind:** code gap. **Closed** 2026-11-05 in `ed0cd62` (lang). Found by app (booking #4).
- **Was:** `examples/apps/booking/probes/clear_day_text_slow.k`: a full `check` ran 15+ minutes; `--quick` was
  instant. One mutant's loop goal never returned: Z3 5.3.3's Diophantine-equation handler (`lp::dioph_eq`) ran
  outside the step limit (`rlimit`), while the same query through the `z3` binary gave up in 1.4 s.
- **Decided by** bet 1 (`check` stays in the inner loop) and F5 (a bound never counts as a catch): `lp.dio` off
  once per process; the step limit set on each fresh context as well; a mutant's proof stops after 4 unknown
  answers (steps, not time, so deterministic). A stopped mutant isn't counted as caught by the prover.
- **Evidence:** `mutation_testing_of_a_quantified_text_loop_finishes` (the probe's definition, inlined); the probe's
  full check now takes 5.7 s (the test 5.3 s). Tester: both digests unchanged (the S2 digest runs full mutation
  testing), goldens unchanged, suite 365/0, fuzz seeds 1–3 0 contradictions, difftest 0 divergences. C6 P1,
  single-threaded, alternated A/B against `e859bc4`: 1.06–1.07 s per definition after, 1.07–1.21 before (no
  slowdown; both over C6's 1 s on this loaded machine, see CHALLENGE.md's C6 note).
- **What remains:** the fix removes the one Z3 path measured to escape the step limit; there's no wall-clock
  guard, so another such path would hang the same way.

### S20. Integers had three widths: prover, interpreter and host store disagreed at the edges
- **Kind:** code gap. **Closed** 2026-11-05: host side `8e8f1b5` (runtime, tests `0a55c5e`), source side and gap
  test `7cbcd28` (lang; host test adjusted in `61994ac`); the host page states the range since `0f4ed45`.
  Found by the tester while testing records over HTTP.
- **Was:** the prover reasoned over unbounded integers, the interpreter in i128, the host's SQLite store in i64.
  A call writing key `i64::MAX + 1` succeeded, the store wrapped it, and replay reported "state differs"; over
  HTTP, `1.5` or 2^63 for an `Int` became 0; literals past i64 parsed.
- **Decided by** F7 (replay must match) and F5 (a fault says so): Int is i64 at run time. A number outside it is
  refused where it is written: a literal, a `const` value or an `eg` value is a parse error naming the range,
  and host input (HTTP 400, `repl`, genesis world) is refused. A call whose stored key or value, event, result
  or refusal payload leaves i64 is a fault, undone, logged and skipped by replay, as the compiled runtime's
  overflow already was.
- **Evidence:** `s20_an_integer_literal_outside_i64_is_refused_where_it_is_written` (i64::MAX itself still
  checks). Tester: 16 positions where a number can be written, `repl` too, all refuse; every host overflow path
  is an error with rollback, and replay matches. Digests and goldens unchanged.
- **What remains:** the prover still reasons over unbounded integers, so a ✔ call can end as a range fault at
  the edge; it fails atomically, and the host page states the range as an assumption. (`eg` value errors carry
  their line since `338fe90`.)

### S11. Tier ○ (runtime-checked only) existed only in the host
- **Kind:** code gap. **Closed** 2026-11-05 in `f160617`, under delegation (.party/DECISIONS.md, 2026-11-05, lang,
  tier ○, and its addendum); test flip and loop digest `68e7b93` (tester); compiled checks `f53e1a0` (runtime);
  review fix `d654dbc`.
- **Was:** the checker's proof page had no ○; the host checked invariants at run time, but the page said `?`.
- **Decided by** F5 (each claim states its real evidence): a module-invariant goal with no proof *and no
  counterexample* is ○, because `kern host` and `kern build` programs check it after every call at the keys the
  call wrote, and undo a call that breaks it as a fault. The wording states the limits (a fault, not a declared
  refusal; the gated host serves it only with `--no-gate`). In review, an invariant that random tests broke
  read "○ checked at run time"; since `d654dbc` it is ✗ with the breaking run as an `eg` line that replays.
- **Evidence:** `f5_runtime_only_tier_exists` passes as an ordinary test (`kern/tests/fundamentals_back.rs`).
  Goldens re-recorded with reasons: p2 and fz2 (`?` → ○, one line each), invcheck (three broken invariants
  ○ → ✗). Loop digest e7234c3d8ea6a48e → 776c5ca29063f921 in `68e7b93`: a dump diff shows exactly two goals,
  unknown → runtime. S2 digest unchanged; fuzz seeds 1–3 0 contradictions. REFERENCE.md 2,300 → 2,344 tokens.
- **What remains:** a compiled program doesn't audit its starting world against the invariants (the host
  does, at startup; HOST.md).

### S21. No iteration limit at run time: a huge loop or sum bound held the host
- **Kind:** code gap. **Closed** 2026-11-05 in `5ffab2d` (runtime); gap test `ec77a7b` (lang), tests `f05dd29`.
- **Was:** the interpreter iterated `for` and `sum` linearly, and `REPLAY_BUDGET` bounded only counterexample
  replay. `sum j from 0 to n` with n = i64::MAX held `kern host`, and its SQLite write lock, for minutes or
  forever, while `ok otherwise` was ✔ (the proof says the call succeeds, not when).
- **Decided by** F7 (one call can't hold the world) and F5 (the limit is stated): at most 10,000,000 loop and
  sum iterations per top-level call, charged as `hi - lo` when a loop or sum starts. Past it the call is a fault,
  undone: in the host `"fault": "limit"` (HTTP 500, logged as an error, skipped by replay), in a compiled program
  a `fault` line. HOST.md and the manifest state the limit.
- **Evidence:** `s21_a_huge_bound_ends_as_a_fault_in_the_host_and_the_compiled_program` passes as an ordinary
  test. Tester: host and compiled program agree on every limit case (exactly the limit succeeds, one more
  faults; a reversed range refunds nothing; sums and callees add up; 2^63 faults at once), replay matches.
  difftest 330,360 calls and fuzzdiff 0 divergences.
- **What remains:** the limit is a fixed constant, not configurable. `kern check` has no limit (random tests use
  small values). As with S20, a ✔ call can end as a fault at the edge; it fails atomically and says so, and
  since `0f4ed45` the host page states the limit as an assumption.

### S12. The host: a reply that never arrived left the request pending
- **Kind:** code gap. **Closed** 2026-11-05 in `f79f085` (runtime); gap test flipped by lang in `96667ab`
  (the two-commit rule for a runtime-only gap); the page states each endpoint's deadline in `43362dd`.
- **Was:** HttpNet's 2 s transport timeout became the `fail` call, but a reply that never reached the host
  (a dropped reply, a crash after flush), or one the module refused, left the request pending forever.
- **Decided by** F7 (every effect recorded, so replay covers it): a request that no ok call to its endpoint's
  reply or fail definition has answered within `timeout N` (a new optional clause on the `.host` line,
  default 300 s) gets its `fail` call once, as an ordinary logged call. The deadline is measured on the
  recorded clock, checked after each call, so it is deterministic and reads no extra clock. A refused reply
  doesn't answer, so it times out too.
- **Evidence:** `s12_a_request_with_no_reply_times_out_into_a_call` passes as an ordinary test and asserts the
  timeout fires exactly once. Hostile network, 1,000 runs, timeout 20: 0 accounts pending once every deadline
  has passed (was 1,952 stuck), 0 faults, 0 replay mismatches. The tester (`c4bd8f9`): a deadline fires once and
  not early, a manual answer suppresses it, a late reply is Stale, replay matches, `timeout 0`/`-1`/`x` is
  refused. No golden, digest or build hash changed. HOST.md and GUIDE.md (`FILE.host`) describe it.
- **What remains:** an idle `serve` fires a deadline only at its next call (no timer). At-least-once
  delivery stays the output contract; whether receivers must deduplicate is a decision, not a test.

### S5, records (first cut: record-typed parameters; one row of S5)
- **Kind:** code gap. **Closed** 2026-11-05 in `b18367a`, under delegation (.party/DECISIONS.md, 2026-11-05,
  lang, records). Fixes found in test: `c586f72` (prover panic on a callee keyed by a record field), `381bfcb`
  (compiled identifiers made injective; a `let p_x` had shadowed `p.x`, so the binary broke a ✔ guarantee).
  Host: a record parameter is one JSON object over HTTP (`38ceceb`).
- **Was:** parameters were Int or Account only; a pair of values was two loose parameters.
- **Decided by** F4/F7 (one meaning on every path): `record Point x:Int y:Int` at column 0; `p:Point` is
  flattened at load time to `p.x:Int p.y:Int`, so run, infer, prove, mutants, the host and the compiled
  program see Ints and Accounts. A header with `p:Point` is 12 tokens against 15 for two loose Ints. F8:
  `eg` lines and counterexamples pass fields positionally, so the round trip holds. F1/F2: views add a
  `record` line per record type in the header.
- **Evidence:** `s5_records_check`, its placeholder the chosen syntax, passes as an ordinary test, with the
  record tests in `kern/tests/shortcomings.rs`. Tester (retest at `6a6671b`): a record program gives the same
  output as its hand-flattened twin; the 13 errors in the decision all fire; six identifier-collision repros
  give 0 divergences and are now a difftest test. Floors at `6a6671b`: digests unchanged, fuzz seeds 1–3
  0 contradictions, difftest 330,360 calls and fuzzdiff 0 divergences. REFERENCE.md 2,344 → 2,438 tokens.
- **What remains:**
  - Not built (decided): records as state values, `-> Point`, nested records, record equality, records in
    `let`s and handler parameters, grouped `eg` arguments.
  - ~~A constructor named `Self` breaks `kern build`~~ fixed in `563177e` (variant `_Self`). Underscore
    constructors (`Too_Big`) fixed in `02e40dd`; difftest test `c674eca`.
  - ~~Positional `eg` arguments that are too few, or grouped `(1 2)`, give a failing example with no reason~~
    fixed in `fb80e41`: a failing `eg` says why (wrong argument count, or the actual after-values;
    `s5_a_failing_eg_says_why`).

### S6, a stated loop frame (the other half of S6; S6 is closed)
- **Kind:** code gap. **Closed** 2026-11-05 in `48dd716`, under delegation (.party/DECISIONS.md, 2026-11-05, lang,
  a stated loop frame). Loop digest and frame generator `d3d1d80` (tester).
- **Was:** the loop frame was inferred and couldn't be stated, so in batch.k a mutant moving a loop bound by one
  (`hi + 1`, `lo - 1`) survived every guarantee.
- **Decided by** F4 (`writes` stays inferred, never written) with F3/F8 (where the author wants the frame as a
  guarantee, it is a claim the prover checks): `keep X` (X unchanged) and `keep X except A to B` (unchanged
  outside [A, B), Int-keyed states). 9 tokens, against 27 for the unbounded ensure it stands for. Rejected:
  counting a mutant that changes the inferred `writes` as caught (it checks nothing anyone wrote).
- **Evidence:** `s6_a_guarantee_kills_the_sweep_one_too_many_mutant` passes as an ordinary test: batch.k's
  `advance_subs` states `keep sub_due except lo to hi`, and its score goes 14/16 → 16/16.
  `s6_a_stated_frame_is_checked_both_ways`: an overrun and a whole-map `keep` on a writer are ✗ with replaying
  counterexamples; a Text map's frame proves. Tester: false frames ✗, true ones ✔ (nested loops, Text), an
  Account range refused; gen_frame catches an off-by-one frame plant. Loop digest 776c5ca29063f921 →
  e90c41b6af1ac4d6, re-recorded after the tester's own dump diff: only `advance_subs` changed (13 → 14 goals,
  the new `keep`, proven). Golden and proof page of examples/batch re-recorded for the same line.
- **What remains (decided):** Account-keyed and ledger frames (`keep ledger except a b`), using a stated frame to
  lift past a callee (S19), and ratcheting the inferred `writes` in the `.strength` baseline. Events inside a loop
  are still flagged, not specified.

### S6, sum (one half of S6; the other half is "S6, a stated loop frame")
- **Kind:** code gap. **Closed** 2026-11-05 in `72d2446`, under delegation (.party/DECISIONS.md, 2026-11-05,
  lang, aggregation; app's request #1). Fuzz generator `6a6671b`; sum bounds `80c3b26`.
- **Was:** no way to state a total or a count, in a spec or a body; `-> Int` functions couldn't read state.
- **Decided by** F2/F5 (exact, no approximation): `sum j from A to B of E [where C]` (11 tokens; with a filter
  15) is one Z3 recursive function per sum, defined from the right, so a loop step is one unfolding. `where`,
  not `when`, so `ensure X = sum … when C` keeps one reading (F1). `-> Int` functions may read state and the
  clock; their symbol takes the arrays they read, so a call before and after a write are different values.
- **Evidence:** `s6_a_sum_invariant_proves`, its placeholder now the chosen syntax, passes as an ordinary test.
  A seeded floor of 60 sum programs: 220 goals proven, 80 refuted and replayed, 0 undecided. The tester's
  sum generator: fuzz seeds 1–3, 827/827 programs, 0 contradictions, a planted sign bug caught. Digests,
  goldens and difftest unchanged. examples/apps/todo's `count_overdue` proves `0 <= result <= hi - lo`
  (after `80c3b26`: interval bounds `m·n <= S <= M·n` as ground facts, no induction; tester's 22 interval traps,
  `34d54a1`, give no false ✔). REFERENCE.md 2,445 → 2,524 tokens (budget 3,500).
  Not used (decided): bounds that hold only under a module invariant.
  Compiled path `c4eb9a1`: fuzzdiff over 150 sum programs and tests/probes/sums.k (24,000 calls) give 0
  divergences; a planted inclusive-bound bug is caught (142 divergences; tests `7d75b0a`).
- **Cost:** sum proofs take about 1.5 s per generated program, against about 0.05 s for loop programs
  (tester, `#TH2UWV33V6`). Counts against bet 1.
- **What remains:** local accumulators in loops, nested sums, calls inside a sum body and `f' a` (decided:
  not now). A sum's iteration is unbounded at run time (S21).

### S5, named constants (one row of S5; the entry stays open for the rest)
- **Kind:** code gap. **Closed** 2026-11-05 in `0acdb54`, under delegation (.party/DECISIONS.md, 2026-11-05,
  lang). Tests `c24cf83`.
- **Was:** no way to name a number; a fee or a limit was a repeated literal.
- **Decided by** F4/F7 (one meaning on every path): `const NAME N` at column 0 (3 tokens, the same as
  `fee = 5`) is substituted into the AST at load time, so run, infer, prove, mutants, the host and the compiled
  program all see the literal, and `*` with a constant stays linear. Design rule 4: a new value changes the
  hash of every user, so its `.why` entries go stale. F1/F2: spec and editor views add one `const NAME N`
  line (4 tokens) per constant they show; the page shows the value, since a human judges the concrete guarantee.
- **Evidence:** `s5_named_constants_check`, rewritten from its placeholder to the real syntax, passes as an
  ordinary test, with `s5_a_constant_is_its_value_to_the_checker_and_its_name_in_views`,
  `s5_a_name_that_would_hide_a_constant_is_an_error` and `s5_a_constant_from_another_module_needs_an_import`.
  The tester's (`c24cf83`): hoisting every literal of the corpus and of 500 fuzzer programs into constants
  gives the same program (`s5_every_literal_hoisted_to_a_constant_means_the_same_program`,
  `every_generated_program_means_the_same_with_its_literals_as_constants`); staleness is exact
  (`s5_changing_a_constant_stales_exactly_the_definitions_that_use_it`); host run and replay agree. At `0acdb54`:
  288 tests pass, both digests and the goldens unchanged, fuzzer seeds 1–3 0 contradictions, difftest and
  fuzzdiff 0 divergences. REFERENCE.md 2,262 → 2,300 tokens.
- **What remains:**
  - Not built (decided): negative values (write `0 - n`), constant expressions, constants in `eg` lines (a
    constant there is a parse error, "bad world item").
  - The decision says a constant named like a handler is an error; handler names are dotted, so
    `const ledger 5` is accepted. It is harmless (`ledger.debit` still resolves), but the decision text and
    the code disagree. Keyword names are rejected unevenly (`need`, `body` are; `const`, `import`, `state`
    aren't).
  - An unparenthesised `for j from lo to lo + cap` with a constant gives "constant cap takes no arguments",
    a misleading message (the literal version is also rejected, with a different one).
  - MCP: `kern_add` refuses `const x 5`, so an agent working only through MCP can't declare a constant (F1/F2).
  - *Resolved 2026-11-05 in `9cac022` (tests `2ddd4ec`):* `kern_add` takes one declaration line (const, record,
    state, channel, invariant); handler roots and declaration words are rejected as constant names, as the
    amended decision says; a compound `for` bound says to parenthesise it, and since `5322fc7` so does a
    compound `sum` bound.

### S8. Mutants were body-only
- **Kind:** code gap. **Closed** 2026-11-05, under delegation.
- **Was:** moving a `need` from the body into the spec removed its mutants. In the ledger tour, transfer
  went from 14 mutants to 10, so v6–v7's 10/10 is a smaller denominator, not more strength
  (examples/ledger-tour/README.md; [FIXES.md](FIXES.md)).
- **Decided by** F8 (a surviving mutant becomes a guarantee or an example, and strength is measured) and
  F5 (each number says what it measures). A spec need isn't body: the runtime enforces it from the spec.
  Mutating it asks a different question: does anything pin the refusal itself? So it gets its own line,
  **spec refusals**, and the body score keeps its meaning. `.strength` baselines and both digests read only
  the body score, and are unchanged.
  - **Mutants:** each spec `need` dropped, and its top-level comparison flipped (`>` → `>=`, `=` → `!=`), as
    for body mutants. Two per need. The body mutant set would give 58 extra mutants on ledger_v7 and 1,201 on
    escrow_s3 (against 22 and 605 body mutants), which would roughly triple a full check (bet 1).
  - **Scoring:** cheapest first, stopping at the first catch: examples, then the prover, then random tests.
    Callers' proofs (`ok otherwise`) aren't re-run, as for body mutants (modular), so a strengthened need is
    caught only by an example.
  - **Output:** `spec refusals: 6/8 changes caught (examples 5, guarantees 1)`, and one `unpinned:` line per
    need with a surviving change. The page says the same in a paragraph under Spec strength.
- **Found:**
  - Across the 18 recorded `check` outputs of the examples, 552 of 772 spec changes are caught: 247 by
    examples, 305 by guarantees. 220 survive (28%). The tour's seven steps are counted separately, so
    transfer's needs are counted in several of them.
  - In the tour's last step, nothing pins transfer's `need amount > 0` (dropped or `>=`). payout's
    `ledger.balance from >= amount + fee` survives `>` (no example at the boundary).
- **Cost (a trade-off, measured):**
  - Best of three, with HEAD and the new build alternated on this machine:

    | Run | Before | After | Change |
    |---|---|---|---|
    | C6 P1 (s per definition) | 0.359 | 0.422 | ×1.17 |
    | C6 P2 (s per definition) | 0.070 | 0.089 | ×1.28 |
    | C6 k=6 (s per definition) | 1.235 | 1.354 | ×1.10 |
    | escrow_s3 `check` (s) | 2.18 | 3.03 | ×1.39 |
    | ledger_large `check` (s) | 0.148 | 0.181 | ×1.22 |
    | ledger_v7 `check` (s) | 0.062 | 0.081 | ×1.30 |

    Those runs use the default 4 jobs. **Single-threaded, as C6's 2026-11-05 addendum measured**, P1 goes
    from 0.849 to 0.899 s per definition (best of three; runs 0.90–0.99 s). That still passes, but with a
    1–10% margin instead of 9–15% (CHALLENGE.md, C6 note). P2 stays far under 60 s.
  - **Tokens:** escrow_s3's CLI `check` report grows from 10,220 to 12,834 (+26%), mostly from 84
    `unpinned:` lines. That counts against C2b's K-full arm. MCP's `kern_check` and the views are unchanged.
    REFERENCE.md grows by one bullet (2,227 → 2,262 tokens).
- **Evidence:** `s8_spec_need_mutants_are_generated`, rewritten from a gap test. Its old assertion was on
  `check::mutants`, which stays body-only by design; it now asserts on `check::spec_mutants` and on
  `Report.spec_mutation`. Also `s8_a_spec_need_is_pinned_by_an_example_that_expects_the_refusal`.
  Goldens gain lines only (331 insertions, 0 deletions); 285 tests pass.
- **What remains:**
  - The ratchet (`kern baseline`) doesn't record spec refusals, so a drop in them isn't flagged.
  - A mutated spec that is still self-consistent remains S3's problem.
  - *Note (2026-11-05, `8b85fc0`):* random worlds now satisfy the module invariants (repaired to defaults where one
    fails, else skipped and counted). Some spec refusals were only "caught" in worlds no call can reach: escrow_s2/s3
    go from 7/8 to 5/8 (`need hold.get id > 0` is unreachable after `need disputed …` under `disputed = 0 or hold > 0`)
    and now show as unpinned. The "Found" counts above are from before that change.

### S18. The page showed a body need's source text even when it read post-write state
- **Kind:** code gap (page wording). **Closed** 2026-11-05, under delegation.
- **Was:** an own-refusal row printed the body need's *source* text. For a need after a write, "refuses
  TooRich when balance of to ≥ 1000" meant the balance *after* the debit and credit, and the row didn't say
  so. The reader never sees the body, so the row read as a pre-state condition and was wrong.
- **Decided by** F3 (the reader judges guarantees without the body) and F5 (a template must not say more
  than the checks established). A row is in the state before the call, or it says it isn't:
  - a need that reads state this call already changed is shown in its lifted pre-state form, when that form
    has one case per read;
  - when the pre-state form splits on aliasing, or there is none (a payload read after the writes, lifting
    stopped, or the form rests on a callee that isn't clean), the row keeps the source text and adds
    "(after this call's earlier changes)";
  - a need before any write, or one that reads no state, is unchanged.
  - The case split loses to the note because it is unreadable as a row. On the probe below it reads
    "(to ≠ from or balance of to − amount + amount ≥ 1000) and (to = from or balance of to + amount ≥ 1000)".
    The page cancels `x − n + n` in the forms it shows (exact on Int).
- **Before → after**, on `kern/tests/shortcomings.rs`'s `S18_SRC`:

  | Definition | Before | After |
  |---|---|---|
  | `cap` (passes `from != to`, then moves `amount` to `to`) | balance of to ≥ 1000 → TooRich | balance of to + amount ≥ 1000 → TooRich |
  | `alias` (the same, without `from != to`) | balance of to ≥ 1000 → TooRich | balance of to ≥ 1000 (after this call's earlier changes) → TooRich |

- **Evidence:** `s18_a_refusal_after_a_write_is_stated_in_the_pre_state_on_the_page` and
  `s18_a_refusal_whose_pre_state_form_splits_on_aliasing_says_it_reads_the_changed_state`. Views, the
  prover and the `.why` anchors are untouched: the change is in `render.rs`, plus a position (`Lifted.at`)
  and `infer::needs_after_writes` that only the page reads. No golden or example page changed, since none
  has an own need after a write. Both digests are unchanged and the suite passes (284).
- **What remains:**
  - A readable case form (`if C then A else B`) would be new syntax, and so a proposal.
  - Lifted lines that rest on an unclean callee are still hidden from views rather than marked. On the page
    they now fall back to the note, which is honest.

### S13. At a leaf, Kern doesn't win
- **Kind:** trade-off. **Accepted** 2026-11-05, under delegation.
- **Evidence:** transfer's editor view is 234 tokens; the TypeScript function alone is about 170.
  Full Kern source for a definition and its callees costs more than TypeScript (purchase 1,258
  against 963). The saving comes only from reading callees as specs
  ([COMPARE.md](COMPARE.md#findings), findings 4–5). Dropping the inferred lines (`uses`, `writes`, `fails`)
  from a leaf's editor view saves 24 of 262 tokens on ledger_v7's transfer (9%, measured 2026-11-05). What
  remains is the spec and the examples, the content a caller relies on.
- **Decided by** F2 and F4: the inferred lines are the checked interface a caller reads instead of the body,
  and a view that drops them by position in the call tree would differ from what callers see. F9 says what
  gets loaded matters more than density; 9% at a leaf doesn't change that.
- **The number stays:** the README's "At a leaf, Kern doesn't win" keeps saying so.

### S17. Lifting stopped after a callee that writes
- **Kind:** code gap. **Closed** 2026-11-05 (by kern-44).
- **Now:** a call to a callee whose ensures pin every written key (`ensure ledger.balance' a = …`,
  `keep ledger.total`) becomes exact writes in the log, so a later need lifts. ledger_v3's page now
  lists payout's second refusal, `balance of from − amount < fee → Insufficient`, which it used to
  leave out.
- **Trust:** a lifted condition that rests on a callee's ensures records that callee:
  - the prover adds it to `relied_on`, so callers are downgraded when the callee isn't clean;
  - views and the page show the line only while every callee it rests on is clean.

  `s17_a_claim_resting_on_a_wrong_pinned_spec_is_at_most_conditional_and_hidden_from_views` showed
  the hole first: without the prover line, a false claim was ✔ and the interpreter found the run
  that breaks it.
- **Evidence:**
  - `lifted_conditions_past_writing_callees_agree_with_the_interpreter`: 200 programs, 0
    contradictions, 330 proven against 234 before.
  - The fuzzer at seeds 1–3 is clean, and both digests are unchanged.
  - The goldens change only where explained: escrow_s2's "Relies on" lines name the transitive
    callee, and ledger_v3 gains the fee row.
- **Why entries:** payout's interface changed, so its why went stale in 5 files. Fredrik confirmed the
  entry still holds on 2026-11-05, and it was re-anchored.
- **What remains:** S19. A marker form for lines that rest on an unproven callee, instead of hiding
  them, is still open (S18's remainder).

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
- **What remains:** S17 and S18, both closed since.

### S2. A cold full `check` was too slow for deep call trees (C6)
- **Kind:** missed target. **Closed** by the rerun in `d959c0f` (2026-11-05).
- **Evidence:** the 2026-11-03 Python run failed both thresholds (P1 16.46 s per definition against
  ≤ 1 s; P2 110 s against ≤ 60 s). The 2026-11-05 rerun of the frozen plan on the Rust toolchain passes
  both: P1 0.85–0.91 s, P2 34.0–39.3 s, over 7 runs. The work was the same: the generator is
  byte-identical and the mutant counts are equal
  ([CHALLENGE.md, C6 addendum 2026-11-05](CHALLENGE.md#c6-prover-stress-run-2026-11-03)).
- **What remains** is S16: P1 passes by about one `if`. Since S8, single-threaded P1 is 0.90–0.99 s
  (CHALLENGE.md, C6 note).

For gaps found and closed before this register
existed, see AUDIT.md's "Status after the fix pass" and ROADMAP_RUN.md's FZ findings.

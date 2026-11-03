# Kern — project context

Kern is a custom language, compiler and runtime designed for AI agents as the
primary authors and readers of code. Humans don't review code; they review a
generated **proof page**. This file carries over the design decisions and findings
from the design conversation, so they don't need re-deriving.

Design fundamentals and the three bets: [docs/FUNDAMENTALS.md](docs/FUNDAMENTALS.md).
Latest audit against them (status per fundamental, ranked gaps):
[docs/AUDIT.md](docs/AUDIT.md). Fix pass with before/after evidence per step:
[docs/FIXES.md](docs/FIXES.md). Scaling an app in three sizes, with a pre-registered scorecard:
[docs/SCALE.md](docs/SCALE.md). Tests: `.venv/bin/pytest` (strict xfails = known gaps).

## Goal (as stated by Fredrik)

A custom way of expressing logical sequences, optimised for an agent:
- The agent reads the **bare minimum** of code tokens.
- From the same code, a **human-readable proof** is generated showing the code can
  be trusted, plus a **lazy-loaded "why"** section.
- A custom language, runtime and compiler are all allowed, and agents can be
  trained on the language.
- Existing languages (Go, TS, Python) only matter if one of them turns out to be the
  human interface. It isn't: the proof page is.

## Core principle

**Verification is what licenses not reading.** An agent may skip a callee's body
only if the callee's interface is machine-checked. So minimal reading and the proof
page are the same mechanism. The prover follows the same rule: calls are verified
*modularly*, from the callee's spec, never its body.

## Design rules (decided)

1. A definition has four layers, stored and loaded separately: **interface**
   (header, `uses`, `fails`), **spec** (`need`/`keep`/`ensure`/`emit`/`ok otherwise`/`eg`), **body**, and
   **why**.
2. **The agent never writes what the compiler can infer.** `uses` (effects) and
   `fails` (failure set) are inferred, transitively through callees.
3. **Nothing correctness-critical may live in `why`.** If it must be obeyed, it is a
   `need`, `keep` or `ensure`. `why` only informs judgement. This removes the
   "hidden constraint" risk of folding comments away.
4. `why` entries are anchored to the definition's hash. If the code changes, they
   are flagged stale and must be confirmed. They never rot silently.
5. **The proof page is rendered by fixed templates from verifier results. It is
   never model-generated.** Code-to-prose generation is circular: it faithfully
   narrates bugs, it is lossy and it varies between runs. The human's job is
   judging whether these are the *right* guarantees.
6. Honesty tiers on the proof page:
   - ✔ proven for all inputs.
   - ! conditional: relies on a callee that is not proven.
   - ✗ refuted, with a counterexample.
   - Planned: ◐ tested-only and ○ runtime-checked-only.
   - Spec strength is the mutation score, measured only when everything passes.
7. **Failure is atomic:** a refusal rolls back the whole call, so guarantees only
   need to hold on success.
8. All effects go through handlers. That enables deterministic replay and
   simulation, and makes effect inference exact.

## Token economics (measured with the Claude tokenizer bundled in `anthropic==0.34.2`, an older one, so numbers are approximate)

- **Use common English keywords that are one token each:** `need`, `ensure`, `keep`,
  `uses`, `fails`, `eg`, `why`, `else`, `body`. Unicode symbols (`⊢ ⊥ ∷ ⟂ ≡ ←`) cost
  2–3 tokens each and lose to words. Don't use them.
- **Juxtaposition application** (`tx.load from`) instead of `tx.load(from)`: −24%
  on a function body.
- **Abbreviating names barely helps** (`amount` is already one token) and costs
  meaning. Keep full words.
- Savings come from removing **ceremony**: imports, error-class boilerplate, effect
  plumbing, repeated types, braces.
- The same `transfer` function in different languages:

  | Version | Tokens |
  |---|---|
  | Effect-TS | 469 |
  | Go | 452 |
  | Rust | 337 |
  | Plain TS | 309 |
  | Python | 280 |
  | Kotlin | 214 |
  | Early Kern sketch | 133–145 |

## Experiments already done (don't repeat)

- **Lossless Go ⇄ dense projection** (an `if err != nil` → `?` rewrite, kept only
  when expanding it gives back the original byte for byte). The round-trip safety
  mechanism worked. Tokens saved on whole real files were only 0.2–2.6% (net/http,
  os, encoding/json, database/sql, Gitea). Error-check blocks are about 17% of
  tokens in Gitea's `services` code. A full rule set might reach 15–30% (estimated,
  not measured). Conclusion: retrofitting an existing language gives modest gains,
  so go clean-slate. Go is at most a compile target.
- **What comments contain** (keyword classifier, rough): constraint/warning
  comments are 33% (net/http) and 14% (Gitea) of comment tokens. Classifying
  comments is the hard part, and regex can't do it. Go `//go:` directives are code,
  not comments. This analysis led to design rules 3 and 4.
- Comments are 12–33% of tokens in real Go code.

## Current prototype (Python + Z3, about 1,200 lines)

```
kern/syntax.py    lexer and parser; AST is plain tuples; code() prints it back
kern/run.py       interpreter; HANDLERS table (ledger/clock/audit); atomic run()
kern/wf.py        well-formedness + type check; runs before anything is proven or run
kern/infer.py     uses / fails / writes / lifted (body-level refusal conditions) inference
kern/prove.py     symbolic execution to Z3; modular calls (callee spec, then
                  havoc the state, then assume its ensures); counterexample -> eg line,
                  replayed in the interpreter
kern/check.py     examples, random property tests, mutation testing
kern/report.py    runs all checks; trust propagation (callers downgraded to
                  "conditional" if a callee is not clean)
kern/views.py     iface / spec / editor views + token counts; English templates
kern/render.py    proof page HTML + terminal output
kern/why.py       .why store, interface-anchored hash, staleness, orphans, obligation lint
kern/baseline.py  ratchet baseline (<module>.strength): flags weakened specs
examples/ledger_v1..v7.k, ledger.why, ledger_v4..v7.why  the demo loop (see README)
examples/ledger_large.k   7 defs, 4-deep chain; F9 token measurements
examples/escrow_s1.k      scale size 1 (docs/SCALE.md): escrow with branches
kern/scorecard.py         `kern score FILE.k`: the per-size scorecard of docs/SCALE.md
tests/test_fundamentals.py  adversarial audit tests, one section per fundamental
```

Run: `python -m kern check examples/ledger_v3.k --why examples/ledger.why --html out/proof.html`.
Dependencies: `z3-solver` (required); `tokenizers` and `anthropic==0.34.2` (optional,
for token counts).

### Known limitations

- The domain is hard-coded: the ledger, clock and audit handlers are written in Python.
- Types: only `Int` and `Account` parameters (plus internal Owner/Bool/Event/Unit); no
  records, no return values. Recursion is rejected (no termination measures yet).
- Bodies are guards plus `if`/`else` blocks (the prover forks a path per branch; paths grow as
  2^ifs per body). No `match` or loops. Specs have `ensure E when C` and `emit … when C`.
- Frames are *inferred* (`writes`), not written.
- `ok otherwise` (opt-in) says a call succeeds whenever its spec needs hold. A caller that
  opts in must restate every refusal (payout's spec view: 97 → 163 tokens); nothing is
  inherited, and every refusing callee must be marked too. Only 1 of 9 existing defs passed
  with it on by default. Moving a `need` into the spec removes its mutants (mutants are
  body-only), so v7 transfer's 10/10 is a smaller denominator, not more strength
  (docs/FIXES.md). Without the marker, refusal mutants are caught only by examples. If a
  body or callee need contradicts the spec needs outright, the claim shows ∅ "never succeeds"
  rather than ✗ (both red, different wording).
- Time: `clock.now` is one value per top-level call (it ticks after each call, refused
  ones included; a bad call doesn't tick). Specs may read it; it has no after-state.
  There is no operation for distinct instants inside one call (add `clock.tick` if
  needed). Example worlds take `now:N`.
- Audit spec (`emit`) is exact-sequence only; no way to say "emits nothing"; `eg`
  can't assert audit events.
- Lifted refusal conditions (`body need`) stop at the first ledger write; later ones
  that read balances aren't visible to callers. Agent views show only a definition's
  *own* body needs (inherited ones measured at ~2x the spec-view tokens).
- Counterexamples from modular proofs may not replay; those show as `?` unconfirmed,
  never ✗. Within one long-running process, Z3's witness choice can depend on earlier
  solver calls, which can flip a mutant between refuted and unconfirmed (CLI runs are
  reproducible).
- Mutation tests use 150 random runs per mutant; the `settle` audit mutant needed 500
  to be caught by random tests (the prover now catches it).
- The obligation lint for `why` is a keyword heuristic.
- Tier ○ (runtime-checked) and record/replay are not implemented (roadmap 4).
- Application arguments must be atoms; nested calls need parentheses. An open
  design question is whether to parse by arity instead, using known interfaces.
- The `.why` file is chosen with `--why`, defaulting to `<module>.why`.

## Roadmap (agreed order)

1. **Language:** records, `match`/branches, loops with invariants (a minimal type checker exists).
   Handlers declared *in Kern* with assumed specs (shown as "assumed" on the proof
   page). ~~Specs reaching the audit log~~ done: `emit Ctor args` (8 tokens; the
   `ensure audit...` forms measured 13–15), exact appended sequence, `_` wildcard.
2. **Store + MCP:** content-addressed definitions (`Def.hash` exists), names mapped
   to hashes. Expose `view` (iface/spec/editor), `why`, `check` and structural edits
   as an MCP server, so an agent works through views instead of files.
3. **Experiment (the central claim):** the same tasks in editor view versus full
   source. Measure tokens, task success, and how often the agent breaks a guarantee
   it never read.
4. **Runtime:** effect handlers with record and replay, compilation to Wasm, proven
   checks removed at compile time.
5. **Training:** generate tasks as specs; `check` is the reward signal.

## Working style

Fredrik works iteratively and wants measurements over assertions, honest
uncertainty, and critical caveats. He mixes Swedish and English. When a design
claim can be tested cheaply, test it and report the numbers, including when they
contradict an earlier claim.

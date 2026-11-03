# Kern — project context

Kern is a custom language, compiler and runtime designed for AI agents as the
primary authors and readers of code. Humans don't review code; they review a
generated **proof page**. This file carries over the design decisions and findings
from the design conversation, so they don't need re-deriving.

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
   (header, `uses`, `fails`), **spec** (`need`/`keep`/`ensure`/`eg`), **body**, and
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
kern/infer.py     uses / fails / callees inference
kern/prove.py     symbolic execution to Z3; modular calls (callee spec, then
                  havoc the state, then assume its ensures); counterexample -> eg line,
                  replayed in the interpreter
kern/check.py     examples, random property tests, mutation testing
kern/report.py    runs all checks; trust propagation (callers downgraded to
                  "conditional" if a callee is not clean)
kern/views.py     iface / spec / editor views + token counts; English templates
kern/render.py    proof page HTML + terminal output
kern/why.py       .why store, hash anchoring, staleness
examples/ledger_v1..v3.k, ledger.why    the demo loop (see README)
```

Run: `python -m kern check examples/ledger_v3.k --why examples/ledger.why --html out/proof.html`.
Dependencies: `z3-solver` (required); `tokenizers` and `anthropic==0.34.2` (optional,
for token counts).

### Known limitations

- The domain is hard-coded: the ledger, clock and audit handlers are written in Python.
- Only `Int` and `Account` types; no type checker beyond arity checks.
- Bodies are straight-line plus guards only: no branches, `match` or loops.
- No frame conditions: an `ensure` can't say "other accounts are unchanged".
- The spec can't describe the audit log, so the 3 surviving mutants in v3 are all
  audit-related.
- Application arguments must be atoms; nested calls need parentheses. An open
  design question is whether to parse by arity instead, using known interfaces.
- The `.why` file is chosen with `--why`, defaulting to `<module>.why`.

## Roadmap (agreed order)

1. **Language:** records, a type checker, `match`/branches, loops with invariants.
   Handlers declared *in Kern* with assumed specs (shown as "assumed" on the proof
   page). A way for specs to reach the audit log, e.g.
   `ensure audit.last' = TransferDone ...`.
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

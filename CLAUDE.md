# Kern — project context

Kern is a custom language, compiler and runtime designed for AI agents as the
primary authors and readers of code. Humans don't review code; they review a
generated **proof page**. This file carries over the design decisions and findings
from the design conversation, so they don't need re-deriving.

**Start with [AGENTS.md](AGENTS.md)** (working rules, including keeping the docs current), the language
reference [docs/REFERENCE.md](docs/REFERENCE.md) and the how-to guide [docs/GUIDE.md](docs/GUIDE.md).

Design fundamentals and the three bets: [docs/FUNDAMENTALS.md](docs/FUNDAMENTALS.md).
Latest audit against them (status per fundamental, ranked gaps):
[docs/research/AUDIT.md](docs/research/AUDIT.md). Fix pass with before/after evidence per step:
[docs/research/FIXES.md](docs/research/FIXES.md). Scaling an app in three sizes, with a pre-registered scorecard:
[docs/research/SCALE.md](docs/research/SCALE.md). How Kern reaches the OS (SQLite state, outbox, recorded clock,
HTTP/console, replay; `kern host`): [docs/HOST.md](docs/HOST.md); next PoC plan with a
pre-registered scorecard: [docs/research/HOST_PLAN.md](docs/research/HOST_PLAN.md). Kern vs behaviour-identical TS and Go,
tokens to write and read: [docs/research/COMPARE.md](docs/research/COMPARE.md) (`experiments/compare/`). Pre-registered challenges
per bet (seeded bugs, cold author, spec gaming, blind proof pages, second domain, prover stress):
[docs/research/CHALLENGE.md](docs/research/CHALLENGE.md). Tests: `cargo test --workspace --release` (`#[should_panic]` tests marked `// gap` = known gaps).

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
   - ◐ tested-only; ○ runtime-checked-only (a module invariant the prover can't decide; `kern host` and
     `kern build` programs check it after every call).
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

## Current prototype (Rust + Z3 toolchain, about 12,500 lines; Rust runtime, about 800)

The toolchain was Python until 2026-11-05. It was ported to Rust module by module, each module checked
against the Python one on recorded runs, and then the Python was deleted (docs/research/RUST_PORT_PLAN.md:
pass conditions, decisions, results). Since Fredrik's decision of 2026-11-05, Python's behaviour is not a
compatibility target: what binds is the fundamentals, the fuzzer, the tests and the golden outputs.

Layout (README.md has the table): **ships** = `kern/` (crate and binary `kern`) + `runtime/` (Cargo workspace
at the root; `Dockerfile` for the container); **not shipped** = `examples/`, `tests/probes/`, `validation/`
(crate `kern-validation`: tools that check the toolchain), `experiments/` (measurements; frozen challenge
material, still Python), `docs/research/` (plans and results). `build/` is generated and ignored. Nothing in
`kern/` or `runtime/` depends on `validation/` or `experiments/`.

```
kern/src/syntax.rs    lexer and parser; AST enums (Expr, Stmt, Def, Module); code() prints it back
kern/src/run.rs       interpreter; HANDLERS table (ledger/clock/audit); atomic run(); the reference semantics
kern/src/wf.rs        well-formedness + type check; runs before anything is proven or run
kern/src/infer.rs     uses / fails / writes / lifted (body-level refusal conditions) inference
kern/src/prove.rs     symbolic execution to Z3 (zz.rs builds the terms); modular calls (callee spec, then
                      havoc the state, then assume its ensures); counterexample -> eg line, replayed in the
                      interpreter; a fresh Z3 context per proof
kern/src/check.rs     examples, random property tests, mutation testing
kern/src/report.rs    runs all checks (analyse); trust propagation (callers downgraded to "conditional" if a
                      callee is not clean); the .kern-cache
kern/src/views.rs     iface / spec / editor views + token counts; English templates
kern/src/render.rs    proof page HTML + terminal output
kern/src/why.rs       .why store, interface-anchored hash, staleness, orphans, obligation lint
kern/src/baseline.rs  ratchet baseline (<module>.strength): flags weakened specs
kern/src/package.rs   packages + imports: a dir with kern.pkg; `import a.b`; one merged Module, per-file visibility
kern/src/host.rs      `kern host` (docs/HOST.md): SQLite state, outbox + channels, recorded clock, net endpoints,
                      HTTP + console front ends, replay, permission manifest, proof gate, `page`
kern/src/mcp.rs       `kern mcp`, the MCP server (stdio): views, why, check, callers, structural edits that
                      re-check callers
kern/src/emit_rust.rs `kern build`: Kern (file or package) -> Rust crate in build/<module>, linked with runtime/
                      (an installed binary carries the kern-rt sources)
kern/src/cli.rs       the `kern` command line (main.rs routes `host` and `mcp`)
kern/src/py*.rs       Python's repr, random and json.dumps, byte for byte: definition hashes, .why anchors and
                      the random test streams kept their Python values, so existing .why files stay valid
runtime/              kern-rt, the Rust runtime crate (src/lib.rs: world, Tx, rollback; src/driver.rs: the binary's CLI)

examples/             one standalone project per directory, each with a README and proof-pages/:
                      ledger-tour/ (ledger_v1..v7 + .why, the demo loop), ledger-large/, escrow/ (escrow_s1..s3, scale
                      sizes, docs/research/SCALE.md), limits/ (host net demo), batch/ (loops), raffle/ (assumed
                      handler), shop/ library/ lottery/ (packages), random-portal/ (a web app on the host)

kern/tests/           fundamentals_front.rs, fundamentals_back.rs, fundamentals_scale.rs: adversarial audit tests,
                      one section per fundamental; host.rs, mcp.rs, package.rs, runtime.rs (compiled program vs
                      interpreter), docs.rs (REFERENCE/GUIDE in step with the code); shortcomings.rs (a gap test per
                      code-gap entry of docs/research/SHORTCOMINGS.md, plus a check that the register's names exist); golden.rs + golden/: recorded
                      `check` output, views and some proof pages per module, the regression floor (re-record with
                      KERN_BLESS=1, and say why in the commit)
tests/probes/         small modules the tests and validation tools use
validation/src/       kern-validation (binaries; validation/README.md): fuzz (soundness fuzzer: random loop/handler
                      programs + template mutants, interpreter as oracle), digest (results digests, S2 and --loops:
                      the floor for prover changes), difftest and fuzzdiff (compiled program vs interpreter),
                      sensitivity (does difftest catch planted runtime/emitter bugs), chains (C6's chain generator), c6 (C6's
                      frozen runner on the Rust toolchain)
validation/tests/     difftest, fuzz, digest (both digests at their recorded values), fuzz_generators,
                      prover_no_answer

experiments/challenge/    C1, C2, C6 (frozen Python; scripts expect the pre-2026-11-04 layout, rerun from d521236)
experiments/compare/      Kern vs TS and Go (docs/research/COMPARE.md)
experiments/bench/        runtime baselines (docs/research/RUNTIME_PLAN.md)
experiments/scale/        the scale scorecard (was `kern score`)

docs/REFERENCE.md         the language reference (≤ 3,500 tokens since 2026-11-05; C2's language tax)
docs/GUIDE.md             how-to: install, tools, file formats, Rust library API, common errors
docs/research/            AUDIT, FIXES, SCALE, COMPARE, CHALLENGE, ROADMAP_RUN, HOST_PLAN, RUNTIME_PLAN, RUST_PORT_PLAN
AGENTS.md                 working rules for agents, including the docs-update rule
```

Run: `kern check examples/ledger-tour/ledger_v3.k --why examples/ledger-tour/ledger.why --html build/proof.html`.
Install: `cargo install --path kern` (needs Z3 5.3.3, e.g. `brew install z3`; or `--features static-z3` to build
it from source), or `docker build -t kern .`. Token counts need the Claude tokenizer file from `anthropic==0.34.2`
(`KERN_TOKENIZER` or `~/.cache/kern/tokenizer.json`). Tests: `cargo test --workspace --release`.

### Known limitations

The register of shortcomings, with evidence, kind and what counts as fixed: docs/research/SHORTCOMINGS.md.

- The domain is hard-coded: the ledger, clock and audit handlers are built into the interpreter. Modules
  can declare `state NAME Int|Account` maps (Int values) and `invariant x:Type COND` (assumed
  and checked at parameter keys; a computed-key write is proven quantified, at every key).
- `need as C args` / `emit as C args` expand a callee's spec at parse time; only right when
  the callee runs first (otherwise the prover refutes it and the caller restates).
- `*` may multiply two variables (nonlinear: Z3 can answer unknown, shown `?`); `/` needs a positive literal on
  the right. Int is i64 at run time
  (host store, `kern build`); a literal, `const` or `eg` value outside i64 is a parse error (S20). The interpreter
  computes in i128 and the prover over unbounded integers, so an i64 overflow in the middle of a call is a run-time fault.
- Types: `Int`, `Account`, `Text` and record parameters (plus internal Owner/Bool/Event/Unit). Text: literals,
  `=`/`!=`, `length`, `+` (join), `state NAME KEY -> Text` maps; Z3 strings in the prover; no substring, search,
  ordering or conversion yet, and no Text in refusal payloads. `-> Int` returns a value
  (set `result`; `eg … -> ok N` asserts it). A total, read-only one is a function: specs may call it (modularly:
  uninterpreted over its arguments and the state it reads, + its ensures). One that writes or refuses is called
  only as a statement, `x = f a`, proven from its spec with a fresh `result` (#ZYGSNMXNW7). Int or record results. Recursion needs `decreases E` on every definition of the cycle
  (proven: `0 <= E(args) < E(params)` at each recursive call); the interpreter stops calls nested deeper than 200.
- Bodies are guards plus `if`/`else` blocks (the prover forks a path per branch; paths grow as
  2^ifs per body) and bounded loops `for k from LO to HI` with `invariant` lines (a proposal, roadmap
  run S4). The loop frame is inferred (a spec may state one: `keep X except lo to hi`); there's no per-iteration emit form. Loops nest; the outer
  loop's frame havocs what an inner loop writes unless keyed by the outer variable. Locals can't be updated in a loop (no accumulators); `sum j from A to B of E [where C]` (a Z3
  recursive function per sum) covers totals and counts, in specs and bodies. `match E` arms (`PAT -> STMT`) are the `if` chain they stand for (n arms, n paths). Specs have `ensure E when C`
  and `emit … when C`.
- Frames are *inferred* (`writes`), not written; a spec may state one as a checked claim, `keep X` or
  `keep X except A to B` (S6), where the frame should be a guarantee.
- `ok otherwise` (opt-in) says a call succeeds whenever its spec needs hold. A caller that
  opts in must restate every refusal (payout's spec view: 97 → 163 tokens); nothing is
  inherited, and every refusing callee must be marked too. Only 1 of 9 existing defs passed
  with it on by default. Moving a `need` into the spec removes its body mutants, so v7 transfer's 10/10 is a
  smaller denominator, not more strength (docs/research/FIXES.md); since S8, spec needs get their own mutants
  (dropped, comparison flipped), reported apart as "spec refusals" (SHORTCOMINGS.md S8). Without the marker, refusal mutants are caught only by examples. If a
  body or callee need contradicts the spec needs outright, the claim shows ∅ "never succeeds"
  rather than ✗ (both red, different wording). A ∅ that rests on something unproven (a refuted loop
  invariant, a callee with a broken spec) shows `!` instead, since the code does succeed (FZ finding 3).
- Time: `clock.now` is the call's time (it ticks after each call, refused ones included; a bad call doesn't
  tick). `clock.tick` moves it on by exactly 1 inside a call (S10); a callee or loop that may tick leaves it later
  or equal. Specs read the start time; it has no after-state. A body need reading `clock.now` after a tick isn't
  lifted. Example worlds take `now:N`.
- Audit spec (`emit`) is exact-sequence only ("emits nothing" is `emit Never when 1 = 0`); an `eg` line's fourth
  section asserts the exact events (S10).
- Lifted refusal conditions (`body need`): direct writes (debit/credit, `x.set`) are rewritten into the
  pre-state, with aliasing split into cases (at most 8); a callee whose ensures pin what it writes
  (`ensure R' = E`, `keep R`) counts as exact writes, and the condition then rests on that callee (relied_on;
  hidden from views unless it is clean); lifting stops at other writing callees, loops with effects and `if`s
  whose branches wrote (SHORTCOMINGS.md S19). The page's "Refuses when" shows a body need after a write in that
  pre-state form when it has one case, else its source text marked "(after this call's earlier changes)" (S18).
  Agent views show only a definition's
  *own* body needs (inherited ones measured at ~2x the spec-view tokens).
- Counterexamples from modular proofs may not replay; those show as `?` unconfirmed, never ✗,
  unless a random-test run broke the same clause: any goal the prover left `?` is then ✗ with that run as its
  `eg` (#J4WKIQY6FL). Results repeat, within one process too: since 2026-11-05 every proof gets a fresh Z3
  context and fresh names (`kern/tests/determinism.rs`; `KERN_Z3_FRESH` switches it off for
  comparisons). In the Python toolchain, Z3's witness choice followed earlier solver calls in the same
  process, which could flip a mutant between refuted and unconfirmed. Across processes, results repeat
  since 2026-11-04: solver limits are Z3 steps (`rlimit`, on each solver and each fresh context; `lp.dio` is off,
  zz::setup, since Z3's Diophantine handler ignored the limit), not wall-clock; a mutant's proof stops after 4 unknown
  answers (prove::MUTANT_UNKNOWNS). The prover no longer
  iterates sets in hash order. Before that, CLI runs were *not* reliably reproducible (FZ finding 6).
- Mutation tests use 150 random runs per mutant; the `settle` audit mutant needed 500
  to be caught by random tests (the prover now catches it).
- The obligation lint for `why` is a keyword heuristic.
- Assumed handlers (`handler rand.below n:Int -> Int` with need/ensure, no body; roadmap run S5, accepted by
  Fredrik 2026-11-04):
  proofs that rely on them are ⊙ (coarse: every proven guarantee of a definition that reaches one).
  The host provides them, checks the spec on every call and records each value for replay. A provider
  with no value for the arguments (e.g. `rand.below 0`) is an atomic error, not a crash.
- `check --quick` skips mutation testing; `.kern-cache/` reuses results by a conservative key. A full
  cold `check` still misses C6's thresholds (docs/research/ROADMAP_RUN.md, S2). `kern.mcp` serves views and
  structural edits to agents.
- Tier ○ (runtime-checked) is shown by `check` and the page for a module invariant the prover neither proved
  nor refuted (SHORTCOMINGS S11); the check runs in the host and, since f53e1a0, in `kern build` programs for
  invariants not proven. Record/replay exist only in the host (`kern host`: SQLite,
  replay, invariants checked at the keys a call wrote, after every call by default; `--skip-proven`
  removes the check for proven definitions, opt-in since Fredrik's decision of 2026-11-04). Output channels (`channel console`,
  `emit console Line x`) are exact per channel, unordered across channels. Network requests are
  `net` events with endpoints in a `.host` file; replies arrive as new calls; a request no ok reply
  answers within its deadline (`; timeout N`, default 300 s of the recorded clock, checked after each call) gets its
  `fail` call once. Delivery is at-least-once. docs/HOST.md has the scorecard.
- Packages (a proposal, roadmap run P): `import a.b` brings a module's names in unqualified; names are unique
  per package; `use ../other` in kern.pkg brings in a sibling package as `other.<module>` (S5; examples
  share examples/money). Missing for
  ordinary programs (gap table in docs/research/ROADMAP_RUN.md, P): lists and decimals (open tasks). A collection
  of accounts is an Int-indexed Account map iterated with `for`/`sum` (no loop over the world's accounts,
  DECISIONS 2026-11-06); "count = 0 means no element matches" is not provable yet (task FPCP2UCC36). Accounts as state values: `state member Int -> Account`, an unset entry is `none` (-1 in the
  prover, every Account parameter assumed another); each use of a non-parameter Account as an account is a
  call-site `pre` goal (only in modules with such states).
- Records: `record Name f:Int|Account|Text|Record …` (nested; a cycle is an error); record-typed parameters,
  flattened to their leaf fields at load time (`syntax::flatten_records`; `l:Line` is `l.a.x l.a.y …`); `eg` and
  counterexamples pass the fields positionally; `p = q` / `p != q` compare every field. Stored records:
  `state order Int -> Order` is one state per field (`order.qty`), whole reads/sets/frames expand at load time
  (a set that reads the record reads every field first); no Account fields in state. `-> Point` returns a
  record (one constructor value in run/prove: `r.x` reads a leaf by position; one fresh value or uninterpreted
  function per leaf in proofs); `r = f a`, `q = p`, `o = order.get k` bind records; `eg … -> ok 1 2`.
- Named constants: `const NAME N` (N ≥ 0) is substituted at load time (`syntax::resolve_consts`), so checker,
  host and compiled program see a literal; views add `const NAME N` lines. No negative values, no constant
  expressions, none in `eg` lines. A name that would hide one (parameter, `let`, loop variable) is an error.
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

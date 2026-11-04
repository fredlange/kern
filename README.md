# Kern — prototype

New here? Read [docs/REFERENCE.md](docs/REFERENCE.md) (the language) and
[docs/GUIDE.md](docs/GUIDE.md) (tools, files, API). Agents: [AGENTS.md](AGENTS.md).

A small language for agent-written code. The agent reads only what it needs, and
humans get a proof page generated from verifier results. The toolchain is Python (about
5,500 lines) with Z3 as the prover. A Rust runtime (about 800 lines) runs compiled programs.

```
pip install -e .                 # the toolchain, its one dependency (z3-solver) and a `kern` command
pip install -e '.[tokens,dev]'   # optional: token counts in `view`, and pytest
python -m kern check  examples/ledger_v3.k --why examples/ledger.why --html build/proof.html
python -m kern view   examples/ledger_v3.k payout --as editor --why examples/ledger.why
python -m kern why    examples/ledger_v3.k transfer --why examples/ledger.why
python -m kern run    examples/ledger_v3.k "transfer A B 100" --world "A:500@u1 B:0@u1"
python -m kern baseline examples/ledger_v4.k     # record guarantee strength; check then flags drops
python -m kern check  examples/shop --quick      # a package: every file, grouped by module
python -m kern.host repl examples/escrow_s2.k --world "A:1000@u1 E:0@u0 B:0@u2 H:0@u9"   # run it for real (docs/HOST.md)
python -m kern.host manifest examples/limits.k   # effects and network endpoints the module can reach
python -m kern.emit_rust examples/escrow_s2.k --build   # compile to a native binary (needs cargo)
```

Tests: `pytest` from the repository root runs the whole suite. `tests/test_fundamentals.py` is the
audit against [docs/FUNDAMENTALS.md](docs/FUNDAMENTALS.md). Known gaps are strict `xfail`s (see
[docs/research/AUDIT.md](docs/research/AUDIT.md); fixes and their before/after evidence are in
[docs/research/FIXES.md](docs/research/FIXES.md)).

## Repository layout

| Path | What it is | Ships? |
|---|---|---|
| `kern/` | the toolchain: parser, checker, prover, views, proof page, host, MCP server, Rust emitter | yes (`pyproject.toml`) |
| `runtime/` | `kern-rt`, the Rust crate every compiled program links with | yes (`runtime/Cargo.toml`) |
| `docs/` | the language reference, the guide, the host design, the fundamentals | docs |
| `examples/` | example programs and generated proof pages ([examples/README.md](examples/README.md)) | no |
| `tests/` | the test suite (`pytest`); `tests/probes/` holds the small modules the tests use | no |
| `validation/` | tools that check the toolchain itself: fuzzer, digests, Rust-vs-interpreter tests ([validation/README.md](validation/README.md)) | no |
| `experiments/` | measurements behind the design claims: challenges, language comparison, benchmarks ([experiments/README.md](experiments/README.md)) | no |
| `docs/research/` | plans, pre-registered scorecards and results ([docs/research/README.md](docs/research/README.md)) | no |
| `build/` | generated: Rust crates and cargo output (ignored by git) | no |

## What a definition is

```
transfer from:Account to:Account amount:Int        header
  need amount > 0 else BadAmount                    refuses with BadAmount
  need ledger.balance from >= amount else Insufficient (amount - ledger.balance from)
  keep ledger.total                                  same value before and after
  ensure ledger.balance' to = ledger.balance to + amount   ' = after the call
  emit TransferDone from to amount clock.now         exactly these audit events (_ = any value)
  ok otherwise                                       optional: succeeds whenever the needs hold
  ensure ledger.balance' to = ledger.balance to when amount = 0   a clause can hold only `when` C
  eg A:500@u1 B:0@u1 | transfer A B 100 -> ok | A:400 B:100   executed example
body
  ledger.debit from amount                          a body may have `need`s too; under
                                                    `ok otherwise` they must never fire
```

- `uses` (effects) and `fails` (failure set) are **inferred**. The agent never writes them.
- `emit` clauses list the exact sequence of events the call appends to the audit log.
  With no `emit` clause the log is unconstrained. It is not inferred from the body: a
  spec projected from the body changes when the body changes, so it could never catch
  a bug.
- A refusal undoes the whole call, so guarantees only have to hold on success.
- `why` entries live in a separate file, anchored to the definition's hash *and* its
  inferred interface. They go stale when the code changes, or when a callee change
  alters what this definition can do or refuse. Entries that read like obligations
  ("must …") are flagged, and orphaned entries are reported.
- Nothing correctness-critical may live in `why`. If it must be obeyed, it is a
  `need`, `keep` or `ensure`.

## What `check` does

| Step | Method |
|---|---|
| Well-formedness, types | names, arity, no shadowing, pure specs, no recursion; Int / Account / Owner / Bool / Event / Unit. Nothing ill-formed is proven or run |
| Effects, failures | inferred from the body, transitively through callees |
| Writes, refusal conditions | inferred: which balances can change (the frame modular proofs use), and the body-level conditions under which a call refuses |
| Examples | run in the interpreter |
| Random tests | 500 random worlds and inputs; every guarantee is checked on each success |
| Proofs | symbolic execution to Z3. Calls are verified *modularly*, from the callee's spec and inferred interface, never its body |
| Refusals | with `ok otherwise`, proven to refuse only where a spec `need` fails |
| Tiers | ✔ proven, ✗ refuted (replayed), ? unconfirmed or no answer, ! relies on a broken callee, ◐ tested only, ∅ never succeeds |
| Trust | a modular proof is downgraded if a callee's guarantees are broken |
| Counterexamples | rendered as `eg` lines and replayed in the interpreter; only a replayed one counts as ✗ |
| Spec strength | mutation testing of the body. The headline is what the *guarantees* catch; examples are reported separately |
| Ratchet | `kern baseline FILE.k` records strength and guarantees; `check` flags "Weaker than baseline" |
| Proof page | fixed templates over the results. No model-written text |

## Agent views

| View | Contents | `payout`, measured |
|---|---|---|
| `iface` | header, uses, writes, fails, why stub | 45 tokens |
| `spec` | + need, keep, ensure, emit, own `body need` lines, proof status | 77 tokens |
| `editor` | own full definition + `spec` of each callee (never callee bodies) | 251 tokens |

(`payout` in `ledger_v3.k`. These were 40/72/222 before `writes` and `body need`
lines were added; see docs/research/FIXES.md.)

## The demo loop

1. `ledger_v1.k`: Z3 refutes `transfer`'s `ensure` clauses with
   `eg A:101@u1 | transfer A A 101 -> ok | A:101` (a self-transfer leaves the
   balance unchanged). `payout` shows as proven only *conditionally*, because it
   relies on `transfer`.
2. `ledger_v2.k`: one line added, `need from != to else SameAccount`. Everything is
   proven. `payout` picks up `SameAccount` in its inferred failures without being
   edited. Its `why` is anchored to its interface, so the v1 page flags payout's why
   as stale even though payout's text is identical. Transfer's guarantees catch 6/13
   mutants (examples 9/13). The rest are about *when* it refuses, its payload, and
   the audit event.
3. `ledger_v3.k`: adds an exact-balance example. Example coverage rises to 10/13;
   guarantee strength stays at 6/13, because an example is not a guarantee. The
   survivors that nothing catches are all about the audit log.
4. `ledger_v4.k`: adds `emit TransferDone from to amount _` to `transfer`, raising
   guarantee strength to 9/13 (everything together: 13/13). The prover refutes all three audit mutants itself, with
   counterexamples confirmed in the interpreter. `payout` adds two `emit` lines and
   proves them from `transfer`'s spec alone. If `transfer` had no `emit` clause,
   `payout`'s would show as unprovable rather than pass. The `.why` entries are
   re-anchored in `ledger_v4.why`. A timestamp mutant (`clock.now` → `0`) still
   survives, because `_` accepts any time.
5. `ledger_v5.k`: the emit clauses say `clock.now` instead of `_`. Time is one value
   per top-level call (`clock.now` reads the same everywhere inside it and advances
   between calls, refused calls included), so the spec can state the timestamp.
   Guarantees catch 10/14 (v4: 9/14); the timestamp mutant is caught. Payout's two
   events share a timestamp; their order is fixed by position in the log.
6. `ledger_v6.k`: transfer moves `need ledger.balance from >= amount else Insufficient …`
   into its spec and adds `ok otherwise`, a proven claim that it refuses for no other
   reason. Payout opts in too and restates all its refusals, but keeps `need fee >= 0`.
   Payout's claim is refuted with `eg … | payout A B B 1 0 -> BadAmount` (transfer
   refuses a zero fee), replayed in the interpreter. In v5 the same bug shows a clean page.
7. `ledger_v7.k`: payout says `fee > 0`, and everything is proven. Its `Insufficient`
   condition is proven only through transfer's `ensure`. The cost is tokens: payout's
   spec view goes from 97 to 163, because a caller that opts in restates its callees'
   refusals. Transfer's 10/10 strength is not an improvement: the mutated `need` left the
   body (see docs/research/FIXES.md).

## Built-in versus missing

The prototype has one hard-coded domain: handlers for `ledger`, `clock` and `audit`
written in Python. It has only `Int` and `Account` types and only straight-line
bodies with guards and `if`/`else` blocks (docs/research/SCALE.md, size 1). A small type checker covers the six built-in types. There are no
written frame conditions: the frame is *inferred* (`writes`), and the page says which
written balances no guarantee describes. A spec can say *when* a call succeeds
only by opting in with `ok otherwise`, and a caller must then restate its callees'
refusals (none are inherited). Within one call there is a single time; a call that needs distinct instants
would need a separate advancing operation (not built).

## Roadmap

1. **Language:** records, `match` and branches, loops with
   invariants, and handlers declared in Kern with *assumed* specs (marked "assumed"
   on the proof page). Specs reaching the audit log is done (`emit`, v4).
2. **Store:** definitions content-addressed by hash (the hash already exists) and
   names mapped to hashes. Expose `view`, `why`, `check` and structural edits as an
   MCP server, so an agent works through the views instead of files.
3. **Experiment:** the same tasks under `editor` view versus full source. Measure
   tokens, task success, and how often the agent breaks a guarantee it never read.
   This tests the central claim.
4. **Runtime:** effect handlers with record and replay, compilation to Wasm, and
   proven checks removed at compile time.
5. **Training:** generate tasks as specs and use `check` as the reward signal.

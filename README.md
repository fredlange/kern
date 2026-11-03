# Kern — prototype

A small language for agent-written code. The agent reads only what it needs, and
humans get a proof page generated from verifier results. Python, about 1,200 lines,
with Z3 as the prover.

```
pip install z3-solver            # required
pip install tokenizers anthropic==0.34.2   # optional: token counts in `view`
python -m kern check  examples/ledger_v3.k --why examples/ledger.why --html out/proof.html
python -m kern view   examples/ledger_v3.k payout --as editor --why examples/ledger.why
python -m kern why    examples/ledger_v3.k transfer --why examples/ledger.why
python -m kern run    examples/ledger_v3.k "transfer A B 100" --world "A:500@u1 B:0@u1"
```

## What a definition is

```
transfer from:Account to:Account amount:Int        header
  need amount > 0 else BadAmount                    refuses with BadAmount
  keep ledger.total                                  same value before and after
  ensure ledger.balance' to = ledger.balance to + amount   ' = after the call
  eg A:500@u1 B:0@u1 | transfer A B 100 -> ok | A:400 B:100   executed example
body
  have = ledger.balance from
  need have >= amount else Insufficient (amount - have)
  ledger.debit from amount
```

- `uses` (effects) and `fails` (failure set) are **inferred**. The agent never writes them.
- A refusal undoes the whole call, so guarantees only have to hold on success.
- `why` entries live in a separate file, anchored to the definition's hash. They go
  stale when the code changes.
- Nothing correctness-critical may live in `why`. If it must be obeyed, it is a
  `need`, `keep` or `ensure`.

## What `check` does

| Step | Method |
|---|---|
| Effects, failures | inferred from the body, transitively through callees |
| Examples | run in the interpreter |
| Random tests | 500 random worlds and inputs; every guarantee is checked on each success |
| Proofs | symbolic execution to Z3. Calls are verified *modularly*, from the callee's spec only |
| Trust | a modular proof is downgraded if a callee's guarantees are broken |
| Counterexamples | rendered as `eg` lines and replayed in the interpreter to confirm |
| Spec strength | mutation testing of the body, measured only once everything passes |
| Proof page | fixed templates over the results. No model-written text |

## Agent views

| View | Contents | `payout`, measured |
|---|---|---|
| `iface` | header, uses, fails, why stub | 40 tokens |
| `spec` | + need, keep, ensure, proof status | 72 tokens |
| `editor` | own full definition + `spec` of each callee (never callee bodies) | 222 tokens |

## The demo loop

1. `ledger_v1.k`: Z3 refutes `transfer`'s `ensure` clauses with
   `eg A:101@u1 | transfer A A 101 -> ok | A:101` (a self-transfer leaves the
   balance unchanged). `payout` shows as proven only *conditionally*, because it
   relies on `transfer`.
2. `ledger_v2.k`: one line added, `need from != to else SameAccount`. Everything is
   proven. `payout` picks up `SameAccount` in its inferred failures without being
   edited, and its `why` notes are flagged stale. Spec strength is 9/13. The
   survivors show that nothing says an exact-balance transfer succeeds, and nothing
   covers the audit event.
3. `ledger_v3.k`: adds an exact-balance example, raising spec strength to 10/13.
   The remaining survivors are all about the audit log, which the spec language
   cannot express yet.

## Built-in versus missing

The prototype has one hard-coded domain: handlers for `ledger`, `clock` and `audit`
written in Python. It has only `Int` and `Account` types and only straight-line
bodies with guards. There is no type checker beyond arity checks and no frame
conditions.

## Roadmap

1. **Language:** a type checker, records, `match` and branches, loops with
   invariants, and handlers declared in Kern with *assumed* specs (marked "assumed"
   on the proof page). Specs also need to reach the audit log, for example
   `ensure audit.last' = TransferDone ...`.
2. **Store:** definitions content-addressed by hash (the hash already exists) and
   names mapped to hashes. Expose `view`, `why`, `check` and structural edits as an
   MCP server, so an agent works through the views instead of files.
3. **Experiment:** the same tasks under `editor` view versus full source. Measure
   tokens, task success, and how often the agent breaks a guarantee it never read.
   This tests the central claim.
4. **Runtime:** effect handlers with record and replay, compilation to Wasm, and
   proven checks removed at compile time.
5. **Training:** generate tasks as specs and use `check` as the reward signal.

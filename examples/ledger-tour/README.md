# Ledger tour

Seven versions of the same two-function ledger, `transfer` and `payout`, each fixing what the
previous version's proof page showed. Read them in order: together they show what `kern check`
finds and how a spec gets stronger.

```
ledger-tour/
├── ledger_v1.k … ledger_v7.k      the seven steps
├── ledger.why                     rationale for v1–v3 (pass it with --why)
├── ledger_v4.why … ledger_v7.why  rationale for v4–v7 (found next to the file)
└── proof-pages/ledger_v1.html …   one proof page per step
```

## Run it

With Kern installed (`cargo install --path <kern repo>/kern`), from this directory:

```text
kern check ledger_v1.k --why ledger.why       # step 1: refuted, with a counterexample
kern check ledger_v2.k --why ledger.why       # step 2: proven
kern view ledger_v7.k payout --as spec        # what a caller of payout reads (163 tokens)
kern why ledger_v3.k transfer --why ledger.why
kern host repl ledger_v7.k --world "A:500@u1 B:0@u1 H:0@u1"
```

## The steps

1. **v1:** Z3 refutes `transfer`'s `ensure` clauses with `transfer A A 101`: a transfer to
   yourself leaves the balance unchanged. `payout` is proven only *conditionally* (!), because
   it relies on `transfer`.
2. **v2:** one line is added, `need from != to else SameAccount`, and everything is proven.
   `payout` picks up `SameAccount` in its inferred failures without being edited. `ledger.why`
   is anchored to the fixed interfaces, so on v1's page both `why` entries show as stale, payout's
   too, although payout's text is the same in both versions.
3. **v3:** adds an exact-balance example. The examples now catch more mutants, but guarantee
   strength doesn't change: an example isn't a guarantee.
4. **v4:** `transfer` says which audit event it emits (`emit TransferDone from to amount _`).
   The prover now catches the audit mutants itself, and `payout` proves its own two events
   from `transfer`'s spec alone.
5. **v5:** the events say `clock.now` instead of `_`, which catches the timestamp mutant.
   Time is one value per top-level call.
6. **v6:** both definitions add `ok otherwise`, a claim that they refuse for no reason except
   the listed ones. `payout`'s claim is **refuted**: `transfer` refuses a zero fee, and
   `payout` only says `fee >= 0`. In v5 the same bug gave a clean page.
7. **v7:** `payout` says `fee > 0`, and everything is proven. The cost is reading: payout's spec
   view grows from 97 to 163 tokens, because it restates its callee's refusals.

## Status

| Step | `kern check` | `transfer`'s spec strength: mutants caught by guarantees / by examples / together |
|---|---|---|
| v1 | not clean: `transfer` refuted, `payout` conditional | not measured (refuted) |
| v2 | clean | 6/14, 9/14, 9/14 |
| v3 | clean | 6/14, 10/14, 10/14 |
| v4 | clean | 9/14, 10/14, 13/14 |
| v5 | clean | 10/14, 10/14, 14/14 |
| v6 | not clean: `payout`'s `ok otherwise` refuted (on purpose; v7 fixes it) | 10/10, 6/10, 10/10 |
| v7 | clean | 10/10, 6/10, 10/10 |

From a full `kern check` (recorded in `kern/tests/golden/`). The 10/10 from v6
on is a smaller denominator, not more strength: the `Insufficient` need moved from the body into
the spec, and mutants are body-only.

The failing steps are the point of the tour: `kern check` exits 1 on v1 and v6.

# Library

A lending library: books with a number of copies, loans with a due date, and fines for late
returns, paid through the ledger. It's a Kern **package** of four files.

```
library/
├── kern.pkg                 `use ../money` (the shared ledger, money.ledger)
├── catalog/
│   └── books.k              state copies; add_copies
├── loans.k                  state due, lent; borrow, give_back
├── fines.k                  state fine; assess (a loop over loans), pay_fine
└── proof-pages/library.html
```

## Run it

With Kern installed (`cargo install --path <kern repo>/kern`), from this directory:

```text
kern check .
kern view . borrow --as spec
kern host repl . --world "M:20@u1 L:0@u9"
kern build .
```

A session:

```text
add_copies 3 1
ok
borrow 9 3
ok
audit #1: Borrowed 9 3 <now>
borrow 10 3
refused NoCopy
give_back 9 3
ok
audit #2: Returned 9 3 <now>
```

Loans are numbered by the caller (`9`, `10`): a Kern operation can't return a new id.

## What it shows

- **Time in specs.** A loan is due at `clock.now + 14`. The prover found a real bug in the first
  version: Kern doesn't promise that the clock is non-negative, so the due date could be
  negative and break the `due >= 0` invariant. `borrow` now says
  `need clock.now >= 0 else BadClock`.
- **A loop over a range of loans.** `assess` adds a fine of 5 to every overdue loan from `lo`
  to `hi`.
- **Cross-file calls.** `pay_fine` (in `fines.k`) reads `due` (from `loans.k`) and pays through
  `transfer` (from `money.ledger`, the shared package `examples/money`).

## Status

`kern check .`: 6 of 6 definitions clean, every guarantee proven.

## Limits

- A loan can't record which member holds it: state values are `Int`, not accounts. So
  `pay_fine` takes the member as an argument.
- `give_back` leaves `lent` set to the old book. It's harmless, since `due` is 0, but a record
  type would make this one value.
- Fines are a flat 5 per `assess` run, not per day overdue.

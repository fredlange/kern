# Ledger, larger

Seven ledger operations in a call chain four deep: `month_end` → `settle` → `payout` →
`transfer`, plus `deposit`, `withdraw` and `rebalance`. Accounts have owners, and a transfer
between different owners is refused.

```
ledger-large/
├── ledger_large.k
└── proof-pages/ledger_large.html
```

## Run it

With Kern installed (`cargo install --path <kern repo>/kern`), from this directory:

```text
kern check ledger_large.k
kern view ledger_large.k month_end --as editor     # the definition plus its callees' specs
kern host repl ledger_large.k --world "A:500@u1 B:0@u1 C:0@u2 H:0@u1"
```

A session:

```text
transfer A B 100
ok
audit #1: TransferDone A B 100 <now>
transfer A C 10
refused SameOwner
settle A B H 100 5
ok
audit #2: TransferDone A B 100 <now>
audit #3: TransferDone A H 5 <now>
audit #4: Settled A B <now>
```

## What it shows

- **Modular proofs.** Each caller is proven from its callees' specs, never their bodies, so an
  agent editing `month_end` needs to read only the specs below it (`--as editor` shows that
  reading).
- **Inferred interfaces.** `month_end` writes no `uses` or `fails` line; both are inferred
  through the whole chain.
- **Refusals in the body.** `transfer` checks the balance in its body (`need have >= amount`),
  not its spec, and callers still learn that it can refuse with `Insufficient`.

## Status

`kern check ledger_large.k`: 7 of 7 definitions clean, every guarantee proven.

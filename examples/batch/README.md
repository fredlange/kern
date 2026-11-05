# Batch jobs

Operations that work through a range of records in one call: sweeping expired holds into a
pool, moving subscription due dates forward, and paying in fixed installments. It's the
example for Kern's **bounded loops**.

```
batch/
├── batch.k                  sweep_expired, advance_subs, installments, sweep_elsewhere, transfer
└── proof-pages/batch.html
```

## Run it

With Kern installed (`cargo install --path <kern repo>/kern`), from this directory:

```text
kern check batch.k
kern view batch.k sweep_expired --as spec
kern host repl batch.k --world "A:100@u1 B:0@u2"
```

A session:

```text
installments A B 3
ok
audit #1: Moved A B 10 <now>
audit #2: Moved A B 10 <now>
audit #3: Moved A B 10 <now>
installments A B 9
refused Insufficient 20
```

The second call would pay 9 × 10 but A has 70 left. It refuses before moving anything, and the
refusal names the shortfall.

## What it shows

- **`for k from LO to HI` with invariants.** Each loop states what holds after every step
  (`invariant … for j from lo to k`). The prover checks that the invariants hold at the start,
  survive one step, and give the spec at the end.
- **Specs over a range.** `ensure hold.get' j = 0 when expiry.get j < clock.now for j from lo to hi`
  says what happens to every record in the range, not just one.
- **The frame.** `sweep_elsewhere` proves that a sweep leaves a hold outside the range alone.
  The loop's frame (what it may write) is inferred, not written.

## Status

`kern check batch.k`: 5 of 5 definitions clean, every guarantee proven.

## Limits

- Loops are bounded ranges of `Int` keys. They can't be nested, and they can't run over a list
  of accounts.
- Loops are still a proposal in Kern.

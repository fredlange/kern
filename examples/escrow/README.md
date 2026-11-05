# Escrow

A marketplace escrow, written in three sizes, each roughly 2.5 times the one before. The
buyer's money waits in an escrow account until the order is captured (paid to the seller, minus
the house fee), released, cancelled or refunded. Each size adds a new kind of demand on the
language, not just more definitions.

```
escrow/
├── escrow_s1.k        size 1, 10 definitions: holds, capture with a fee, refunds, splits; `if`/`else`
├── escrow_s2.k        size 2, 26 definitions: orders, disputes, frozen accounts, daily limits;
│                      declared state with invariants, `need as` / `emit as`
├── escrow_s3.k        size 3, 55 definitions: subscriptions, loyalty points, chargebacks,
│                      holdbacks, VAT, carts; pure functions (`-> Int`) used in specs, `distinct`
└── proof-pages/       escrow_s1.html, escrow_s2.html, escrow_s3.html
```

## Run it

With Kern installed (`cargo install --path <kern repo>/kern`), from this directory:

```text
kern check escrow_s1.k
kern check escrow_s3.k --quick        # skip mutation testing: 0.5 s instead of 4.8 s (2026-11-05)
kern view escrow_s2.k purchase_order --as spec
kern host repl escrow_s1.k --world "A:1000@u1 E:0@u0 B:0@u2 H:0@u9"
kern host serve escrow_s2.k --db escrow.db --world "A:1000@u1 E:0@u0 B:0@u2 H:0@u9"
kern build escrow_s2.k
```

A session with size 1:

```text
reserve A E 300
ok
audit #1: Moved A E 300 <now>
audit #2: Reserved A 300 <now>
capture E B H 200 10
ok
audit #3: Moved E B 190 <now>
audit #4: Moved E H 10 <now>
audit #5: Captured B 200 <now>
release E A 100
ok
audit #6: Moved E A 100 <now>
audit #7: Released A 100 <now>
```

## What it shows

- **Branches.** A spec clause can hold only in some cases (`ensure … when C`, `emit … when C`),
  and the prover checks each path through the `if`s.
- **Declared state and invariants** (size 2): `hold`, `disputed`, `frozen`, `limit` and `spent`
  are maps. Invariants such as "a disputed order still has money on hold" and "balances never go
  negative" are proven for every definition that writes them.
- **Reusing a callee's spec** (size 2): `need as reserve …` and `emit as reserve …` copy a
  callee's refusals and events into the caller's spec, instead of retyping them.
- **Pure functions in specs** (size 3): `fee_for`, `vat_of` and `points_for` compute values that
  both bodies and specs use.

## Status

| Size | Definitions | `kern check` |
|---|---|---|
| 1 | 10 | 10 clean |
| 2 | 26 | 26 clean |
| 3 | 55 | 55 clean |

How reading cost, check time and spec strength changed from size to size is measured in
`docs/research/SCALE.md` in the Kern repository.

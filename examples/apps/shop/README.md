# shop: catalog + orders, two modules in one package

Stock and prices live in their own package, `../catalog` (`kern.pkg` says `use ../catalog`); orders in `orders.k` (`import catalog.stock`), each with a phase
(`const placed 1`, `paid 2`, `shipped 3`, `cancelled 4`) advanced by `match`. Imports across packages used since b71848b; recursion not yet landed.

```
shop/
├── kern.pkg             `use ../catalog`
├── orders.k             place, advance (match on the phase), cancel (restocks)
└── proof-pages/shop.html
```

## Run it

```text
cargo run --release -p kern -- check examples/apps/shop
cargo run --release -p kern -- build examples/apps/shop
```

## What `check` says (HEAD at recording: 8b85fc0, `--cache ''`)

| definition | status | spec strength |
|---|---|---|
| `restock`, `take`, `set_price` | All guarantees proven | 8/8, 5/5, 3/3; spec refusals all caught |
| `place` | All guarantees proven (stock reduced, order recorded, exact `emit`) | 26/26; spec refusals 12/12 |
| `advance` (`match`) | All guarantees proven (`phase' = phase + 1`) | 12/12 |
| `cancel` (calls `restock`) | **Checked at run time only** (○) | invariant on stock at a computed key |

`kern build` compiles the package. What the app found:
- `cancel`'s spec first omitted that it calls `restock`, so its `emit` was wrong (✗, replayed): the exact event
  sequence is `Restocked …, Cancelled …`.
- `place` accepted item 0 until a cross-map invariant made the prover refute it.
- `orders.k` states `phase = 0 or item_of > 0` and `phase = 0 or qty_of > 0` as module invariants (cross-map, read at one key). Until 8b85fc0
  random tests built worlds that broke them and `cancel` read "Broken guarantees" with every guarantee ✔ (request 0, closed by
  lang); the `need` workaround is gone.
- Random testing reaches success rarely: `advance` 0/500, `place` 1/500 (worlds must satisfy the invariants and the needs); the
  proofs carry these definitions, and `advance` went from 25/500 to 0/500 with the repair.

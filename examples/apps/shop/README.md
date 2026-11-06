# shop: catalog + orders, two modules in one package

Stock and prices live in their own package, `../catalog` (`kern.pkg` says `use ../catalog`); orders in `orders.k` (`import catalog.stock`), each with a phase
(`const placed 1`, `paid 2`, `shipped 3`, `cancelled 4`) advanced by `match`. Imports across packages (b71848b) and recursion with `decreases` (3700f8d) are both used.

```
shop/
├── kern.pkg             `use ../catalog`
├── orders.k             place, advance (match on the phase), cancel (restocks), quote (price * qty + shipping)
├── cart.k               order lines as a list: add_line, empty_cart, lines, line_item, cart_value (`state cart Int -> List`)
├── shipping.k           parcels (recursive, `decreases weight`) and shipping
└── proof-pages/shop.html
```

## Run it

```text
cargo run --release -p kern -- check examples/apps/shop
cargo run --release -p kern -- build examples/apps/shop
```

## What `check` says (recorded at 64c39c0, built from a clean checkout of that commit, `--cache ''`)

| definition | status | spec strength |
|---|---|---|
| `restock`, `take`, `set_price` | All guarantees proven | 8/8, 5/5, 3/3; spec refusals all caught |
| `place` | All guarantees proven (stock reduced, order recorded, exact `emit`) | 26/26; spec refusals 12/12 |
| `advance` (`match`) | All guarantees proven (`phase' = phase + 1`) | 12/12 |
| `cancel` (calls `restock`) | All guarantees proven (the stock invariant at a computed key proves since 4b44b2f; it was ○ before) | 4/4 |
| `quote` | All guarantees proven | 3/3 |
| `parcels` (recursive) | All guarantees proven, incl. `measure decreases weight` and the closed form `(weight + 4) / 5` | 11/12; the survivor `<=` → `<` is equivalent (weight 5 gives 1 either way) |
| `shipping` | All guarantees proven | 2/2 |
| `add_line` (`cart.get' c = append (cart.get c) item`, length +1, last element), `empty_cart` | All guarantees proven | 4/4, 1/1; spec refusals 8/8, 2/2 |
| `lines`, `cart_value` (`sum i … of price.get (at (cart.get c) i)`) | All guarantees proven | 0/0 |
| `line_item` (guarded `at`) | All guarantees proven | 8/10 |

`kern build` compiles the package. What the app found:
- Lists (792389e) fit order lines: `invariant c:Int length (cart.get c) <= max_lines` is proven for `add_line`, which
  appends and refuses a full cart; sums over `at xs i` work. Two gaps: `cart_value` cannot prove `result >= 0` although
  `price.get k >= 0` is an invariant (counterexample has price -1: the invariant isn't assumed at the computed key
  `at xs i` inside a sum), and `ensure result = at xs i when i >= 0 and i < length xs` reads `? pre at … is inside the list`
  (the `when` doesn't guard the `at` in the same line), so `line_item` states the out-of-range case only.
- `cancel`'s spec first omitted that it calls `restock`, so its `emit` was wrong (✗, replayed): the exact event
  sequence is `Restocked …, Cancelled …`.
- `place` accepted item 0 until a cross-map invariant made the prover refute it.
- `orders.k` states `phase = 0 or item_of > 0` and `phase = 0 or qty_of > 0` as module invariants (cross-map, read at one key). Until 8b85fc0
  random tests built worlds that broke them and `cancel` read "Broken guarantees" with every guarantee ✔ (request 0, closed by
  lang); the `need` workaround is gone.
- Random testing reaches success rarely: `advance` 0/500, `place` 1/500 (worlds must satisfy the invariants and the needs); the
  proofs carry these definitions, and `advance` went from 25/500 to 0/500 with the repair.
- `parcels` first had a `need weight >= 0`: refused, because `-> Int` functions are total. It now returns 0 for weight <= 0.
  The prover then proved the closed form `(weight + 4) / 5` by induction over the recursion, and refutes a wrong one
  (`result <= weight / 5` gives `✗ … parcels 1`) and a bad measure (`decreases 0 - weight` reads `? measure`).

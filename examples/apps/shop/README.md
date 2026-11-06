# shop: catalog + orders, two modules in one package

Stock and prices live in their own package, `../catalog` (`kern.pkg` says `use ../catalog`); orders in `orders.k` (`import catalog.stock`), each with a phase
(`const placed 1`, `paid 2`, `shipped 3`, `cancelled 4`) advanced by `match`. Imports across packages (b71848b) and recursion with `decreases` (3700f8d) are both used.

```
shop/
├── kern.pkg             `use ../catalog`
├── orders.k             place, advance (match on the phase), cancel (restocks), quote (price * qty + shipping)
├── cart.k               order lines as a list: add_line, empty_cart, lines, line_item, cart_value (`state cart Int -> List`)
├── pricing.k            Decimal unit prices (`state unit Int -> Decimal`) and Account-valued buyers (`state buyer Int -> Account`)
├── shipping.k           parcels (recursive, `decreases weight`) and shipping
└── proof-pages/shop.html
```

## Run it

```text
cargo run --release -p kern -- check examples/apps/shop
cargo run --release -p kern -- build examples/apps/shop
```

## What `check` says (recorded at f2933ff, built from a clean checkout of that commit, `--cache ''`)

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
| `lines`, `cart_value` (`sum i … of price.get (at (cart.get c) i)`, `result >= 0` from the price invariant) | All guarantees proven | 0/0 |
| `set_unit`, `line_total`, `discounted` (Decimal: `unit.get item * qty * (100 - pct) / 100`) | All guarantees proven | 1/1, 2/2, 10/10 |
| `claim`, `release` (`state buyer Int -> Account`, `none`) | All guarantees proven | 1/1, 1/1; spec refusals 4/4, 2/2 |
| `line_item` (guarded `at`, in-range case in the spec) | All guarantees proven | 10/10 |

`kern build` compiles the package. What the app found:
- Decimals and Account-valued state (both used here, `kern build` compiles the package): `unit` is a parallel Decimal price
  (`unit.get item * qty`, exact to 0.0001, `/` rounds down); the Int `price` in `catalog` is unchanged, so the shop now has two
  price notions. Not wired into `place`/`quote`. `buyer.get order = none` guards a claim and `buyer.set order none` clears it.
  A first `claim` example failed with "account 'A' for `who` is not in the world": an `eg` that passes an Account must
  declare it in the world (`A:0@u1`), also in refusal examples.
- Lists (792389e) fit order lines: `invariant c:Int length (cart.get c) <= max_lines` is proven for `add_line`, which
  appends and refuses a full cart; sums over `at xs i` work. Two gaps, both closed by lang: `cart_value` could not prove `result >= 0` although
  `price.get k >= 0` is an invariant (the invariant wasn't assumed at the computed key `at xs i` inside a sum; df05911 does), and the `when`-guarded `at` in `line_item` was `?` until 9e0c650, which now proves it.
- `cancel`'s spec first omitted that it calls `restock`, so its `emit` was wrong (✗, replayed): the exact event
  sequence is `Restocked …, Cancelled …`.
- `place` accepted item 0 until a cross-map invariant made the prover refute it.
- `orders.k` states `phase = 0 or item_of > 0` and `phase = 0 or qty_of > 0` as module invariants (cross-map, read at one key). Until 8b85fc0
  random tests built worlds that broke them and `cancel` read "Broken guarantees" with every guarantee ✔ (request 0, closed by
  lang); the `need` workaround is gone.
- Random testing reaches success: since 7aaecdb half the runs start near the `eg` lines. `advance` 0 -> 52 of 1000 runs, `place`
  1 -> 62, `cancel` 7 -> 147 (the per-guarantee strength of `advance` went from prover-only to 12/12 with random tests too).
- `parcels` first had a `need weight >= 0`: refused, because `-> Int` functions are total. It now returns 0 for weight <= 0.
  The prover then proved the closed form `(weight + 4) / 5` by induction over the recursion, and refutes a wrong one
  (`result <= weight / 5` gives `✗ … parcels 1`) and a bad measure (`decreases 0 - weight` reads `? measure`).

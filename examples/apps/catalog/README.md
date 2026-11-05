# catalog: stock and prices, a package of its own

`stock.k`: `restock`, `take`, `set_price` over `state stock` and `state price` (invariants: both >= 0).
Used by `../shop` through `use ../catalog` in its `kern.pkg`; it checks and builds standalone too
(module `stock` here, `catalog.stock` inside shop: the same file under two module names).

```text
cargo run --release -p kern -- check examples/apps/catalog
cargo run --release -p kern -- build examples/apps/catalog
```

Recorded at 8576908 (`--cache ''`): `restock`, `take`, `set_price` All guarantees proven, spec refusals 4/4 each.
`kern build` compiles it. Proof page: `proof-pages/catalog.html`.

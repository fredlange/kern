# Shop

A small web-shop backend: a catalogue with prices and stock, discounts over a range of items,
and orders that take payment and remove stock in one step. It's a Kern **package**: four files
that import each other.

```
shop/
├── kern.pkg                 marks this directory as a package
├── orders.k                 buy, refund
├── inventory/
│   ├── catalog.k            state price, stock; set_price, restock, restock_range
│   └── promotions.k         discount_range (10% off every item in a range)
├── money/
│   └── ledger.k             transfer
└── proof-pages/shop.html    the generated proof page
```

## Run it

With Kern installed (`cargo install --path <kern repo>/kern`; see the repository README), from this directory:

```text
kern check .                         # prove everything, run the examples, measure spec strength
kern check . --html proof-pages/shop.html
kern view . buy --as spec            # what a caller of `buy` reads
kern host repl . --world "A:100@u1 S:0@u2"
kern build .                         # a native binary (needs cargo)
```

A session (`<now>` is the call's time):

```text
set_price 7 30
ok
restock 7 2
ok
buy A S 7
ok
audit #1: Moved A S 30 <now>
audit #2: Sold 7 A <now>
buy A S 7
ok
buy A S 7
refused OutOfStock
state
{"balances": {"A": 40, "S": 60}, "state": {"price": {"7": 30}, "stock": {"7": 0}}}
```

## What it shows

- **Imports.** `orders.k` imports `money.ledger` and `inventory.catalog`; a file can use only
  what it declares or imports.
- **One call, one transaction.** `buy` pays and removes stock. If either part refuses, nothing
  changes.
- **Restated refusals.** `buy` says `need as transfer buyer shop (price.get item)`, so its spec
  lists every way the payment can fail, and the prover checks that list against `transfer`.
- **Loops with invariants.** `restock_range` and `discount_range` change every item from `lo`
  to `hi`, with an invariant saying what holds after each step.
- **Invariants on state.** Price and stock never go negative, which is proven for every
  definition that writes them.

## Status

`kern check .`: 7 of 7 definitions clean, every guarantee proven.

## Limits

- Prices are per item id. Kern has no records, so an item is a set of `Int`-keyed maps, not
  one value.
- An order total can't be "price × quantity": Kern multiplies only by a literal.
- `money/ledger.k` is a copy of the same file in `library/` and `lottery/`, because packages
  can't import from each other yet.

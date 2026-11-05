# money: the shared ledger package

`ledger.k` defines `transfer`, the guarded move of money between two accounts. It is a package of its own
(`kern.pkg`), so other packages use it instead of copying it: `shop/`, `library/` and `lottery/` each have
`use ../money` in their `kern.pkg` and `import money.ledger` where they call `transfer`.

```text
money/
├── kern.pkg     marks this directory as a package
└── ledger.k     transfer
```

Checking a package that uses it checks `money.ledger` too: `kern check examples/shop`.

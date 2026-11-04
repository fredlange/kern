# Examples

Kern programs. Several tests, validation tools and frozen experiments read these files by path,
so they keep their names.

| Files | What they show |
|---|---|
| `ledger_v1.k` … `ledger_v7.k`, `ledger*.why` | the demo loop: a ledger fixed step by step (see the README) |
| `ledger_large.k` | 7 definitions, a 4-deep call chain |
| `escrow_s1.k`, `escrow_s2.k`, `escrow_s3.k` | one app in three sizes: branches; declared state and invariants; pure functions (55 definitions) |
| `limits.k`, `limits.host` | network requests from the host: limits fetched from a risk service |
| `batch.k` | bounded loops with invariants |
| `raffle.k` | an assumed handler (`rand.below`) |
| `shop/`, `library/`, `lottery/` | packages: several files, `import`, loops, an assumed handler (lottery) |
| `proof-pages/` | proof pages generated from the examples above (`kern check --html`) |

Check one: `.venv/bin/python -m kern check examples/shop --quick`.

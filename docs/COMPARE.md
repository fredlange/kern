# Kern vs TypeScript vs Go: one program, same behaviour

`examples/escrow_s1.k` (10 definitions, call depth up to 4) translated by hand into
idiomatic TypeScript and Go, with tests for the same 20 examples. Code and scripts: `compare/`.

```
.venv/bin/python compare/diff.py [SEED] [N]   # behaviour: Kern interpreter vs TS vs Go (node >= 23, go)
.venv/bin/python compare/tokens.py            # token counts
```

The `platform` files (ledger, audit, atomic rollback) correspond to Kern's handlers and
`run()`, and are not counted.

## Same behaviour

- On 28,000 random cases (seeds 1–3), all three agree on the outcome, including the
  refusal and its arguments, the final balances and the exact event sequence.
- The cases cover every outcome: ok, BadAmount, BadFee, BadPart, Insufficient and SameAccount.
- To get there, TypeScript and Go keep Kern's refusal *order*. Where a callee already
  refuses the same way, they omit the guard, as an idiomatic author would.

## Writing (tokens, measured 2026-11-03)

| | Kern | TypeScript | Go |
|---|---|---|---|
| executable logic (header, `need`s, body) | **820** | 1435 | 1660 |
| the same 20 examples / tests | **658** | 1400 | 1580 |
| checked spec (`keep`/`ensure`/`emit`/`ok otherwise`) | 710 | — | — |
| total | **2267** | 2895 | 3311 |

## Reading (tokens; TypeScript and Go have no views, so a reader loads the function, its transitive callees and the types)

| def | callees | Kern spec view | Kern editor view | Kern full source* | TS | Go |
|---|---|---|---|---|---|---|
| transfer | 0 | 115 | 234 | 212 | 447 | 522 |
| move | 1 | 127 | 328 | 403 | 521 | 604 |
| reserve | 1 | 129 | 314 | 388 | 508 | 592 |
| release | 1 | 132 | 294 | 368 | 508 | 592 |
| capture | 2 | 266 | 661 | 794 | 726 | 861 |
| purchase | 4 | 253 | 714 | 1258 | 963 | 1127 |
| cancel | 2 | 124 | 302 | 515 | 567 | 660 |
| refund | 4 | 207 | 868 | 1260 | 1004 | 1162 |
| split | 2 | 151 | 373 | 623 | 640 | 749 |
| tip | 2 | 122 | 324 | 578 | 581 | 673 |
| **median** | | **130** | **326** | | **574** | **666** |

\* the definition plus every transitive callee, as full Kern source with no views.

## Findings

1. **Kern states the same behaviour in 43% fewer tokens than TypeScript and 51% fewer than
   Go.** It still totals 22–32% less while carrying 710 tokens of proven spec that the
   others have no counterpart for.
2. **Calling something costs 4–5× less to read** (spec view 130 against 574/666), and what
   you read is proven. The TypeScript alternative is the signature (median about 30
   tokens), which says nothing about refusals, effects or events.
3. **Editing costs 43–51% less to read** (median 326 against 574/666). For definitions with
   2 or more callees the saving is 24–36% (517 against 683/805).
4. **Views are what make Kern cheaper to read.** Full Kern source for a definition and its
   callees costs *more* than TypeScript (purchase 1258 against 963), because every callee
   carries spec and examples.
5. **At a leaf, Kern doesn't win.** `transfer`'s editor view is 234 tokens. TypeScript's 447
   includes the shared 276-token type block; the function alone is about 170.

## Not measured, and probably decisive

- **The cost of no training data.** The only Kern reference an agent gets is CLAUDE.md,
  about 3,200 tokens per session. It pays back after about 7 callee reads through spec
  views. How often an agent writes Kern that parses, type-checks and proves on the first
  try is unknown; TypeScript and Go are written fluently.
- **What the tokens buy.** Kern: 51 guarantees proven for all inputs, 99% mutation
  strength. TypeScript and Go: 20 example tests.

## Caveats

- One author (the same agent) wrote all three versions.
- Ten definitions, in a ledger domain that suits Kern.
- The tokenizer is the older one CLAUDE.md mentions.
- Reading cost is a token proxy, not measured task success.

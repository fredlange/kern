# C1 bug catalogue (frozen before any bug is planted)

Each bug is one behavioural change, applied to `examples/escrow_s1.k`, `compare/ts/escrow.ts`
and `compare/go/escrow/escrow.go`. "Kern where" says where the change has to go in Kern:
- **body:** counts toward P1;
- **spec:** it edits a `need`, and is reported separately;
- **none:** it can't be expressed in Kern.

The patches implementing these descriptions are written after this file is committed.

| id | category | change | Kern where |
|---|---|---|---|
| B01 | wrong account | capture (from 100 up) pays the seller's share to `house` instead of `seller` | body |
| B02 | wrong account | partial refund credits `part` to `seller` instead of `buyer` | body |
| B03 | wrong account | release transfers from escrow back to `escrow` instead of to `buyer` | body |
| B04 | boundary | capture's no-fee branch is `amount <= 100` instead of `amount < 100` | body |
| B05 | arithmetic | capture moves `fee + 1` to the house | body |
| B06 | arithmetic | split gives `b` `amount - part + 1` | body |
| B07 | events | capture doesn't emit `Captured` | body |
| B08 | events | reserve emits `Reserved` before the transfer's `Moved` | body |
| B09 | events | tip emits `Tipped` with amount `0` | body |
| B10 | guard | refund drops `part <= amount` (keeps `part > 0`) | spec |
| B11 | guard | move allows a negative amount (doing nothing) | spec |
| B12 | refusal args | `Insufficient` reports `amount` instead of the shortfall | spec |
| B13 | refusal reason | capture reports a bad fee as `BadAmount` instead of `BadFee` | spec |
| B14 | atomicity | purchase moves money before a check that can still refuse (TS/Go: `reserve` before the buyer-funds and `Overdrawn` checks) | none: Kern's spec needs always run before the body |
| B15 | conditions | capture's fee branch is taken from `amount >= 10` instead of `amount >= 100` | body |

## Reserve list (used in order if a bug above is not live)

- **R1:** tip moves the amount from seller to buyer (body).
- **R2:** split gives `a` `part + 1` (body).
- **R3:** cancel emits `Cancelled` for `escrow` instead of `buyer` (body).
- **R4 (atomicity, if B14 is not live):** purchase ignores capture's refusal after reserving
  (TS: try/catch; Go: discard the error). Kern: none.

## Blind bugs

There are 10 more, produced by a fresh `general-purpose` subagent with the frozen prompt in
`blind_prompt.md`. It sees only `compare/ts/escrow.ts` and `platform.ts`, with no specs,
tests or Kern. They are translated into Kern and Go afterwards, and Kern-where is decided
by where that logic lives in `escrow_s1.k`.

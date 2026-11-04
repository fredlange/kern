# C2 tasks (frozen before any run)

All three tasks apply to the escrow service (`escrow_s1`: transfer, move, reserve, release,
capture, purchase, cancel, refund, split, tip). Each run gets one task. These texts are given to
the agent verbatim, after the arm's preamble.

## T1: a new operation

Add an operation `settle escrow buyer seller house amount part fee`. The escrow account holds
`amount` for an order:

- Pay `part` of it to the seller exactly as `capture escrow seller house part fee` does (same
  fee rules, same refusals, same events).
- Then release the remaining `amount - part` to the buyer exactly as `release escrow buyer
  (amount - part)` does.
- When `part = amount`, nothing is released (no release events).
- Refuse with `BadPart` unless `0 < part <= amount`. That check comes first; after it, refusals
  are whatever capture, then release, would give.

The audit events must be exactly capture's events followed by release's events.

## T2: a rule change in a callee

From now on, for `amount >= 1000`, the fee the house takes in `capture` is capped at 50. If the
`fee` argument is above 50, the house gets 50 and the seller gets `amount - 50`. Every caller of
capture must stay correct with the new rule, and so must its audit events. Nothing else changes.

## T3: a change with consequences

`move` must refuse with `SameAccount` whenever `from = to`, even when `amount = 0`. (Today
moving 0 to the same account succeeds and does nothing.) Every other operation must keep its
current behaviour exactly.

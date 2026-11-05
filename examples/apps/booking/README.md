# booking: rooms × days

A booking board: 5 rooms × 7 days, stored flat in one `state slot Int` at key `room * days + day` (0 = free,
else the guest id). Uses `const`, a `record Booking`, sums, loops and `invariant s:Int slot.get s >= 0` on the computed key. Guests have an Int id and a `Text` name (`state holder Int -> Text`).

```
booking/
├── booking.k                         book, cancel, clear_day (slot and name), clear_all (nested loops), seat (variable width), nights_booked, guest_nights, busy_on
└── proof-pages/booking.html
```

## Run it

```text
cargo run --release -p kern -- check examples/apps/booking/booking.k
cargo run --release -p kern -- build examples/apps/booking/booking.k
```

## What `check` says (recorded at 14e8e67 (clean tree), `--cache ''`)

| definition | status | spec strength |
|---|---|---|
| `book` (record with a Text field) | All guarantees proven (slot and name set, every other slot unchanged) | 25/25; spec refusals 14/14 |
| `cancel` | All guarantees proven | 22/24; spec refusals 12/12 |
| `clear_day` (loop over rooms, clears slot and name) | All guarantees proven | 14/16: survivors are loop bounds |
| `clear_all` (nested loops) | All guarantees proven | 14/22 (prover 10): bound and index mutants survive |
| `seat` (`room * width + day`, variable × variable) | All guarantees proven | 14/14; spec refusals 8/8 |
| `nights_booked`, `guest_nights`, `busy_on` (sums) | All guarantees proven | 0/0, nothing to mutate |

`kern build` compiles it. Findings that shaped the file:
- `clear_day` first forgot the guest names; its spec said nothing about `holder`, so nothing flagged it.
- With the names in the same loop, the full `check` once ran 15+ minutes (SHORTCOMINGS S22); since ed0cd62 the whole
  file checks in 9 s, so `forget_names` is folded back into `clear_day`.

## What it can't do yet

- No `mod`, no constant expressions (`slots` is `rooms * days` by hand).

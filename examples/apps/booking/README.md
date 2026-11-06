# booking: rooms × days

A booking board: 5 rooms × 7 days, stored flat in one `state stay Int -> Booking` at key `room * days + day` (guest 0 = free,
else the guest id). A stay is a stored record (`record Booking room day guest:Int name:Text`), one map per field
(`stay.guest`, `stay.name`). Also `const`, sums, loops and `invariant s:Int stay.guest.get s >= 0` on the computed key.

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

## What `check` says (recorded at 6506a42 (built from a clean checkout of that commit), `--cache ''`)

| definition | status | spec strength |
|---|---|---|
| `book` (record with a Text field) | All guarantees proven (`ensure stay.get' k = b`, the whole record; every other key unchanged) | 45/45; spec refusals 14/14 |
| `cancel` | All guarantees proven | 24/24; spec refusals 12/12 |
| `clear_day` (loop over rooms, clears slot and name) | All guarantees proven | 14/16: survivors are loop bounds |
| `clear_all` (nested loops) | All guarantees proven | 14/22 (prover 10): bound and index mutants survive |
| `seat` (`room * width + day`, variable × variable) | All guarantees proven | 14/14; spec refusals 8/8 |
| `nights_booked`, `guest_nights`, `busy_on` (sums) | All guarantees proven | 0/0, nothing to mutate |

`kern build` compiles it. Findings that shaped the file:
- Records in state (6506a42): the two parallel maps `slot` and `holder` became one `state stay Int -> Booking`.
  `book` now states `stay.get' k = b` and `stay.set k b` writes it; the proof expands to all four fields. Cost: the
  record carries `room` and `day`, which the key already encodes, so two fields are written and never read.
  There is no way to build a record value in a body (`stay.set k (Booking …)`), so `cancel` and the clears still write
  field by field (`stay.guest.set k 0`, `stay.name.set k ""`).
- `clear_day` first forgot the guest names; its spec said nothing about the names, so nothing flagged it.
- With the names in the same loop, the full `check` once ran 15+ minutes (SHORTCOMINGS S22); since ed0cd62 the whole
  file checks in 9 s, so `forget_names` is folded back into `clear_day`.

## What it can't do yet

- No `mod`, no constant expressions (`slots` is `rooms * days` by hand).
- No record constructor in bodies or specs: a cleared stay can't be stated or set as one value.

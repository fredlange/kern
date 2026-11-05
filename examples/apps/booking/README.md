# booking: rooms × days

A booking board: 5 rooms × 7 days, stored flat in one `state slot Int` at key `room * days + day` (0 = free,
else the guest id). Uses `const`, a `record Booking`, sums and a loop. Guests have an Int id and a `Text` name (`state holder Int -> Text`).

```
booking/
├── booking.k                         book, cancel, clear_day, forget_names, clear_all (nested loops), seat (variable width), nights_booked, guest_nights, busy_on
├── probes/clear_day_text_slow.k      a clear_day whose full `check` does not finish (below)
├── probes/booking_with_invariant.k   the same plus `invariant s:Int slot.get s >= 0` (see below)
└── proof-pages/booking.html
```

## Run it

```text
cargo run --release -p kern -- check examples/apps/booking/booking.k
cargo run --release -p kern -- build examples/apps/booking/booking.k
```

## What `check` says (recorded at 59c9e66, `--cache ''`)

| definition | status | spec strength |
|---|---|---|
| `book` (record with a Text field) | All guarantees proven (slot and name set, every other slot unchanged) | 25/25; spec refusals 14/14 |
| `cancel` | All guarantees proven | 22/24; spec refusals 12/12 |
| `clear_day`, `forget_names` (loops over rooms) | All guarantees proven | 10/12, 6/11: survivors are loop bounds |
| `clear_all` (nested loops) | All guarantees proven | 10/22 (prover 1): bound and index mutants survive |
| `seat` (`room * width + day`, variable × variable) | All guarantees proven | 14/14; spec refusals 8/8 |
| `nights_booked`, `guest_nights`, `busy_on` (sums) | All guarantees proven | 0/0, nothing to mutate |

`kern build` compiles it. Findings that shaped the file:
- `clear_day` first forgot the guest names; its spec said nothing about `holder`, so nothing flagged it. The
  names are now cleared by `forget_names`.
- Putting both in one `clear_day` makes the full `check` run for 15+ minutes (`--quick` is instant):
  `probes/clear_day_text_slow.k`. So they are two definitions.

## What it can't do yet

- **Module invariant on a computed key:** `probes/booking_with_invariant.k` is "Checked at run time only" (○)
  and callers' proofs become `!`; `booking.k` has no invariant. Request in `.party/requests.md`.
- No `mod`, no constant expressions (`slots` is `rooms * days` by hand).

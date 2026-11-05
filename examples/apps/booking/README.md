# booking: rooms × days

A booking board: 5 rooms × 7 days, stored flat in one `state slot Int` at key `room * days + day` (0 = free,
else the guest id). Uses `const`, a `record Booking`, sums and a loop. Guests are Int ids: no `Text` yet.

```
booking/
├── booking.k                         book, cancel, clear_day, nights_booked, guest_nights, busy_on
├── probes/booking_with_invariant.k   the same plus `invariant s:Int slot.get s >= 0` (see below)
└── proof-pages/booking.html
```

## Run it

```text
cargo run --release -p kern -- check examples/apps/booking/booking.k
cargo run --release -p kern -- build examples/apps/booking/booking.k
```

## What `check` says (HEAD at recording: cb38e8f, `--cache ''`)

| definition | status | spec strength |
|---|---|---|
| `book` (record parameter) | All guarantees proven (target slot set, every other slot unchanged) | 17/17; spec refusals 12/12 |
| `cancel` | All guarantees proven | 16/16; spec refusals 12/12 |
| `clear_day` (loop over rooms) | All guarantees proven (column cleared, other slots unchanged) | 10/12; 2 survivors, below |
| `nights_booked`, `guest_nights`, `busy_on` (sums) | All guarantees proven (`0 <= result <= bound`) | 0/0, nothing to mutate |

`kern build` compiles it. `clear_day`'s survivors are the loop bounds (`0` → `0 - 1`, `5` → `5 + 1`): the
extra writes fall outside the board, which the frames (`s` in 0..`slots`) don't cover.

## What it can't do yet

- **Module invariant on a computed key:** `probes/booking_with_invariant.k` checks as "Checked at run time only"
  (○) for `book` and `cancel`, and `clear_day`'s proven guarantees become `!`. That is why `booking.k` has no
  invariant. Request 1 in `.party/requests.md`.
- No `mod`, no constant expressions (`slots` is `rooms * days` by hand), no `Text`.

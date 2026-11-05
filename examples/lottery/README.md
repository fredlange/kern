# Lottery

Tickets bought into a pot, a loyalty bonus for frequent players, and a draw that pays one of
three players at random. It's a Kern **package** of four files. The randomness comes from an
**assumed handler**: Kern states what a random number is, and the host provides one.

```
lottery/
├── kern.pkg
├── chance.k                 handler rand.below n -> Int (assumed: 0 <= result < n)
├── tickets.k                state tickets; buy_ticket, loyalty_bonus (a loop)
├── draw.k                   draw: pays a random one of three players
├── money/
│   └── ledger.k             transfer
└── proof-pages/lottery.html
```

## Run it

With Kern installed (`cargo install --path <kern repo>/kern`), from this directory:

```text
kern check .
kern host repl . --world "B:25@u1 P:0@u0 A:0@u2 C:0@u3 D:0@u4"
kern host repl . --db lottery.db --world "…"     # then: kern host replay . --db lottery.db
kern build .
```

A session (the winner is random):

```text
buy_ticket 4 B P
ok
audit #1: Moved B P 10 <now>
buy_ticket 4 B P
ok
draw P A C D 15
ok
audit #3: Moved P C 15 <now>
audit #4: Won C 15 <now>
```

## What it shows

- **Assumed handlers.** `rand.below` has a spec but no body. Proofs that rely on it are marked
  ⊙ ("assuming the handler's spec") on the proof page, never ✔.
- **The host checks the assumption at run time.** Every random value is checked against the
  handler's spec and recorded, so `replay` gives the same draw again.
- **A loop.** `loyalty_bonus` gives one extra ticket to every player in a range who holds five
  or more.

## Status

`kern check .`: 4 of 4 definitions clean. Every guarantee of `draw` is ⊙: it rests on the assumed
handler. The one ✔ in `draw` is that it calls `rand.below` with a valid bound. The other definitions
are all ✔.

## Limits

- The draw picks among exactly three players: Kern can't loop over a list of accounts.
- Players are numbered by the caller, and a ticket count can't hold the player's account.

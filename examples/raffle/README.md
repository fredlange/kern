# Raffle

Paying a pot to one of three players at random, and picking a random number in a range. It's the
smallest example of an **assumed handler**: Kern states what a random number is, and the host
provides it.

```
raffle/
├── raffle.k                 handler rand.below; draw_winner, pick_order, transfer
└── proof-pages/raffle.html
```

```text
handler rand.below n:Int -> Int
  need n > 0 else BadBound
  ensure result >= 0 and result < n
```

## Run it

With Kern installed (`cargo install --path <kern repo>/kern`), from this directory:

```text
kern check raffle.k
kern host repl raffle.k --world "P:100@u0 A:0@u1 B:0@u2 C:0@u3"
```

A session (the winner and the pick are random):

```text
draw_winner P A B C 30
ok
audit #1: Moved P C 30 <now>
audit #2: Won C 30 <now>
pick_order 5 9
ok
audit #3: Picked 6 <now>
```

## What it shows

- **Proofs under an assumption.** Everything `draw_winner` and `pick_order` guarantee rests on
  `rand.below`'s spec, so those guarantees are ⊙ on the proof page, never ✔. What is ✔ is
  that every call to `rand.below` passes a valid bound.
- **The host keeps the assumption honest.** Every value the host provides is checked against
  the spec and recorded, so a recorded run replays exactly.
- **`emit` with a wildcard.** `emit Won _ amount clock.now` says a `Won` event happens without
  saying who wins.

## Status

`kern check raffle.k`: 3 of 3 definitions clean. `transfer` is ✔; `draw_winner` and
`pick_order` are ⊙, apart from the bound check.

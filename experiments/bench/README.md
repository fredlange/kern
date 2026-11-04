# Baseline benchmarks (docs/research/RUNTIME_PLAN.md, "Baseline")

The same workload in every language: escrow_s2's `transfer` over 1M accounts of 1000, driven
by one xorshift64 call stream (seed 88172645463325252):

- `from = x % N`
- `to = (x >> 20) % N`
- `amount = (x >> 40) % 1500 - 10`

Every implementation must print the same checksum, `sum(balance[i] * (i + 1))`:

| Calls | Checksum |
|---|---|
| 100k | 500008344233268 |
| 20M | 500262077749564 |

| File | What it is | Run |
|---|---|---|
| `kernhost.py` | kern/host.py over in-memory SQLite, plus run.py | `python kernhost.py CALLS` |
| `plain.py` | Python written as a compiler would emit it | `python plain.py CALLS` |
| `rs/` | Rust, hand-written | `cargo run --release -- CALLS [THREADS]` |
| `go/` | Go, hand-written | `go run . CALLS [THREADS]` |
| `transfer.c` | C, hand-written, plus the durable-log group-commit test | `cc -O2 transfer.c && ./a.out LOGFILE [CALLS]` |

`kernhost.py` and `plain.py` hard-code the author's repo path and venv. Edit them before
running them elsewhere.

The generated program runs the same stream: `build/target/release/escrow_s2 bench transfer
1000000 20000000`.

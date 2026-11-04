# Kern runtime plan: compile Kern to Rust

Status: 2026-11-04, R1 done (C1, C2, C3, P1 pass; M1 fails), R2 next. Owner: kern-90 session. The thresholds below were written
**before** any runtime code ran; they are not edited after a run (add a dated note instead).

## Decision (Fredrik, 2026-11-04)

- A Kern program becomes **one native binary**. `kern build` emits Rust source for the
  module and links it with a hand-written runtime crate, `kern-rt`. There's no intermediate
  language and no interpreter at run time.
- **No SQL.** State lives in memory as flat arrays. Durability comes from an append-only
  command log with group commit, plus snapshots. Recovery is snapshot + log replay.
- **Fast and multi-threaded.** Calls whose inferred key sets don't overlap run in parallel.
  The log order is the serial order, so replay stays exact.
- **Rust**, chosen over Go, C and Python on the measurements below.
- The **Python toolchain stays** (parse, wf, infer, prove, check, render). `kern/run.py`
  stays the **reference semantics** forever. The runtime is wrong whenever the two disagree,
  until shown otherwise (AGENTS.md).

## Baseline (measured 2026-11-04, Apple M5 Max)

The workload is escrow_s2's `transfer` over 1M accounts, driven by the same xorshift call
stream in every language. All five implementations gave the identical checksum on a
100k-call run. The scripts are in the kern-90 scratchpad and will be copied into
`runtime/bench/` in R1.

| Implementation | Per call | Bytes per account |
|---|---|---|
| Kern today (`host.py`, in-memory SQLite, log + outbox) | 54.8 µs | 47 (SQLite) |
| …of which `run.py` interpreter | 26 µs | |
| Plain Python, compiled-style | 1.0–1.3 µs | 40.5 |
| Go, hand-written | 18–26 ns | 8.5–8.8 |
| Rust, hand-written | 10–15 ns | 8.0 |
| C, hand-written | 8–10 ns | 8 |

- **Threads.** Each thread owned a shard and no call crossed shards, so this is the best
  case. Rust: 124 → 1,052 → 1,586 M calls/s at 1 → 8 → 18 threads. Go: 82 → 723 → 1,082.
- **Durability.** C, appending a 24-byte record per call, with `F_FULLFSYNC` once per batch:

  | Calls per sync | Calls/s |
  |---|---|
  | 1 | 324 |
  | 1,000 | 320k |
  | 10,000 | 2.8M |

  Batch latency stays at about 3–3.5 ms. With durability on, the log dominates, not the
  language.
- **Int width.** The prover reasons about unbounded integers, but SQLite stores `i64`. A
  `deposit` that pushes a balance past 2^63 raises an unhandled `OverflowError` in the
  Python host. It is atomic, but it's neither a refusal nor a defined error.

## Architecture

```text
x.k --kern check (gate)--> runtime/emit_rust.py --> build/x/src/module.rs ┐
                                                    runtime/kern-rt/      ├─ cargo --> build/x/.../x
                                                    build/x/src/main.rs   ┘
```

**`module.rs` (generated):**
- one Rust function per definition;
- `Refusal` and `Event` enums, built from the inferred `fails` and the constructors;
- the `DEFS` table: name, params, hash, `uses`;
- per-definition key sets for the scheduler;
- the dispatch.

The specs (`keep`, `ensure`, `emit`) are proven, so they compile to nothing. `need`s are
behaviour: they stay.

**`kern-rt` (hand-written, one crate for every app):**
- the world;
- atomic calls with an undo log;
- checked `i64` arithmetic;
- the clock;
- providers for assumed handlers, with recording;
- the driver (R1);
- later: the log, snapshots, the scheduler, HTTP/console front ends, outbox delivery.

### Semantics contract (the reference is `kern/run.py`)

| Kern | Runtime |
|---|---|
| `Int` | `i64`. `+ - *` are checked. **Overflow is a fault:** an atomic error, rolled back, with no clock tick and no refusal. That's a deliberate divergence from run.py, which has unbounded ints. The differential test keeps values in range, and overflow gets its own tests. |
| `/` (positive literal) | Rounds toward minus infinity (`div_euclid`), as Z3 and Python do. Rust's `/` truncates. |
| Refusal (`need … else C args`) | The payload is evaluated only when the need fails. A callee's refusal refuses the caller. The whole call rolls back. The clock ticks. |
| Runtime error (`KernError` in run.py) | A fault: rolled back, no tick. |
| `clock.now` | One value per top-level call. It ticks after `ok` and after refusals. |
| Events | `audit` and each declared channel, each exact and in order. Dropped on refusal or fault. |
| `state X Account` / `state X Int` | A missing key reads as 0. Account maps are dense vectors; Int maps are hash maps. |
| Assumed handlers | A provider supplies the value. Its `need`s and `ensure`s are checked at run time (a violation is a fault). Every value is recorded for replay. |
| Pure functions (`-> Int`) | They return `result`. |
| Arguments at the boundary | Arity, Account existence and Int type are checked before anything runs. A failure is a bad call, not a fault. |

## Phases, with exit conditions

**R1 – generator and executor core** (now):
- `runtime/emit_rust.py` and `runtime/kern-rt`;
- a driver binary that runs a script of `world` / `call` / `obs` lines and prints canonical
  outcomes;
- a Python harness that runs the same script through `run.py`.

The exit conditions are C1–C3 and P1 below.

**R2 – durability:**
- an append-only log with framing and checksums, group commit, snapshots, recovery and
  `replay`;
- the data directory is bound to the module's definition hashes.

The exit conditions are C4–C6 and P2.

**R3 – parallel scheduler:**
- a sequencer assigns log positions and clock values;
- workers run calls whose key sets don't overlap;
- calls with computed keys take an exclusive lock on the whole map.

The exit conditions are C7 and P3.

**R4 – front ends and effects:**
- HTTP and console;
- channels and outbox delivery (at least once);
- `net` endpoints;
- module invariants checked at the written keys (tier ○, on by default, as in the Python
  host);
- `manifest`.

`tests/test_host.py`'s behaviours get ported to black-box tests that run against both hosts.
The exit conditions are C8 and B1.

**R5 – domain out of the runtime.** `ledger` / `clock` / `audit` are built into `kern-rt`, as
they are in run.py. Moving them into a Kern prelude is a language task. It doesn't block
R1–R4 and is tracked here so the hard-coding isn't forgotten.

## Pre-registered scorecard

| ID | Condition | Pass |
|---|---|---|
| C1 | Every `examples/*.k` module (except those using features R1 doesn't cover, listed explicitly) generates, compiles without warnings, and passes all its `eg` lines in the binary | all listed modules |
| C2 | Differential test against run.py: random worlds, random calls per definition (same world, same call sequence, same provider values). Outcome, refusal payload, events per channel, final world and clock all match | 0 divergences in ≥ 1,000,000 calls in total (≥ 2,000 per definition) |
| C3 | Overflow: a call that leaves `i64` is a fault, the world is unchanged, the clock didn't tick | tests pass |
| C4 | Crash injection: `kill -9` at random points under load; the recovered state equals the replay of the durable log prefix | 200/200 |
| C5 | A log recorded by the Python host, replayed by the Rust runtime, gives an identical state (modules using only features both support) | identical on all of them |
| C6 | A data directory written by a different module version (definition hashes differ) is refused at startup | refused |
| C7 | Parallel run = serial replay of its own log | 1,000/1,000 randomized schedules (threads, batch sizes, conflict rates) |
| C8 | The host behaviours (atomicity, gate, invariants, at-least-once, replay tamper, HTTP codes) pass against the Rust host | all ported tests |
| P1 | Generated `transfer`, in memory, one core, 1M accounts | ≤ 25 ns/call (the hand-written baseline is 10–15) |
| P2 | Durable, group commit, `F_FULLFSYNC` | ≥ 200k calls/s with p99 call latency ≤ 5 ms |
| P3 | Through the scheduler, 8 threads, transfers over 1M accounts with ≤ 1% conflicting calls, in memory | ≥ 10M calls/s, and ≥ 4× the scheduler's own 1-thread throughput |
| M1 | Resident memory per account (balance, owner, name index), 1M accounts | ≤ 96 bytes (Python host: 244 + SQLite) |
| B1 | Release binary for escrow_s3 with HTTP; cold start on an empty data dir | ≤ 5 MB; ≤ 50 ms |

**Stop rule.** If C2 finds a divergence, record it, fix the generator or runtime (never
`run.py` to match), and re-run the full C2. If a performance threshold fails, report it as
failed, with the profile, before any tuning run. Tuning runs are reported separately.

**Not claimed.** The prover's guarantees are about `run.py`'s semantics. The runtime inherits
them only as far as C1–C3 and C7 show equivalence: that's testing, not a proof of the
compiler. A verified compiler is out of scope.

## Coordination

`kern/` is hashed by C2 (`report.tool_hash`), so the generator lives in `runtime/` until
kern-8b says C2 is done. (Done 2026-11-04. kern-8b asks that the move wait until Fredrik has seen the C2
results, so the tree he reviews matches the runs.) After that it moves to `kern/emit_rust.py` with a `kern build` CLI
command, and its GUIDE.md entry lands with it (the docs rule).

## Results

(Filled in per phase, with dates. A failed run is reported like a passed one.)

### R1, 2026-11-04: C1 ✔, C2 ✔, C3 ✔, P1 ✔, M1 ✗

**What exists:**
- `runtime/emit_rust.py`: the generator, 371 lines.
- `runtime/kern-rt`: the runtime, 385 lines in `lib.rs` and 239 in `driver.rs`.
- `runtime/difftest.py`: the differential harness.
- `runtime/tests/`: `test_runtime.py`, `sensitivity.py` and `probes.k`.
- `runtime/bench/`: the baselines.

Commands:
- `python runtime/emit_rust.py X.k --build` produces `runtime/target/release/X`.
- The binary has three modes: `X defs`, `X run SCRIPT` and `X bench DEF ACCOUNTS CALLS`.

**C1 ✔.** All 14 `examples/*.k` modules generate and compile with `-D warnings`.
- None of them needed an R1 exclusion.
- The generated module carries an `allow(...)` list for unused names, so "without warnings" means
  without warnings outside that list.
- All 211 `eg` lines pass, judged on the binary's own output against each eg's outcome and
  after-world.

**C2 ✔.** 0 divergences in **1,005,211** calls.
- Run: `difftest.py --per-def 7500 --seed 20261104`, over 14 modules plus `probes.k`, 142
  definitions. Each definition is guaranteed at least 3,750 calls (half the calls go round-robin;
  the rest are random), about 7,500 on average.
- run.py took 18 s in total; the binaries took 2.4 s, most of it process start.

**The harness was tested before its 0 was believed.** `runtime/tests/sensitivity.py` plants 13
known bugs, one at a time, and requires the harness to fail on each:
- in kern-rt: no clock tick on a refusal, events kept on a refusal, truncating division, three
  kinds of skipped rollback, debit that adds, `total` missing an account;
- in the generator: `>=` emitted as `>`, arguments evaluated right to left, a wrong refusal
  payload, the `else` branch dropped.

The first run missed 7 of the 12:
- **2 were a bug in the check itself.** The generator was imported once, so a planted generator
  bug never ran. Fixed by reloading it per plant.
- **5 were real coverage holes.** No example module ever:
  - refuses after a map write;
  - uses `ledger.total` in a body;
  - has a refusal with two or more payload values;
  - draws two provider values in one argument list.

`runtime/tests/probes.k` reaches all five paths, and also produces faults, which the examples
never did. **Now 13 of 13 are caught**, including a 13th plant added after review (channel events emitted on `audit`). The "else dropped" plant is caught by the build failing,
not by the diff.

**C2 extension (2026-11-04, after kern-8b's C2 ended): kern/fuzz.py's programs.** Command:
`python runtime/fuzzdiff.py`, with seed 1: 300 loop programs, 200 handler programs and 177
template mutants.
- 664 programs compiled, all into one binary; 13 weren't well-formed and were skipped; 0 were
  unsupported.
- **0 divergences in 78,533 calls.** Outcomes: 50,007 ok, 22,966 refused, 4,195 faults and
  1,365 bad calls; 14,964 calls drew provider values.
- run.py used fuzz.py's spec-respecting provider, and the binary replayed its values.
- One cargo build took 67 s; running every program took 13 s.

**C3 ✔** (`runtime/tests/test_runtime.py`, 7 tests). Overflow is a fault: the earlier write is
undone, the clock doesn't tick, and the run.py divergence is asserted rather than hidden. Also
tested: a refusal ticks and a bad call doesn't; floor division matches (−7/2 = −4).

**P1 ✔.** Generated `transfer` runs at **14.5–14.9 ns/call** over 1M accounts (3 runs), against
a threshold of ≤ 25.
- The checksum is **identical** to the hand-written baselines (500262077749564).
- Heavier definitions in escrow_s3: `purchase` (reserve + capture, up to 3 transfers) runs at
  21.6 ns; `open_order` at 15.4 ns.

**M1 ✗.** **172 bytes/account** in escrow_s2 and 162 in ledger_v1 (no state maps), against a
threshold of ≤ 96. This is the profile, recorded before any tuning:
- the account name is stored twice: `Vec<String>` plus a `HashMap<String, u32>` key, each about
  40 bytes plus hash-table slack;
- `bal` is 8 bytes and `owner` is 4;
- vector doubling inflates peak RSS during the build-up.

The tuning idea, for a separate run reported as tuning: store names once in an arena and index
by hash → `u32`. Expected about 40 bytes, not yet measured.

**Not done in R1, and not hidden:**
- `emit_rust.py` checks well-formedness only. The **proof gate** (refusing unclean definitions)
  isn't wired in yet; it moves to R4 with the manifest.
- Module invariants aren't checked at run time.
- `keys()` for the scheduler isn't emitted yet (R3).
- Fixed after review, not a gap: observations are recorded after the handler's `ensure`s pass, as in run.py. That was fixed in
  R1 after review; it matters for the R2 log and for C5.
- Nested constructors (an event inside an event) are rejected with `Unsupported`, never
  mistranslated.
- The differential worlds keep values small, so run.py never leaves `i64`. Overflow is covered
  only by the targeted C3 tests.

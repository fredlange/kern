# Validation: checking the toolchain itself

These tools check that Kern's prover, checker and Rust runtime tell the truth. They aren't part of
the distribution (`kern/` and `runtime/`), and they aren't unit tests. They live in this
directory's crate `kern-validation` (`src/`, one binary per tool). They run longer, are run by hand
after changes to the prover or the runtime, and report numbers into
[docs/research/](../docs/research/). `validation/tests/` runs small, fast versions of some of them with
every `cargo test`.

Until 2026-11-05 each tool also had a Python original, and the Rust toolchain was checked against the
Python one by oracle scripts. Both went with the Python toolchain; the last record of what the oracles
said is in docs/research/RUST_PORT_PLAN.md. The JSON fixtures recorded from CPython 3.9.6
(`kern/tests/pyrandom_vectors.json`, `validation/tests/fuzz_generators.json`) stay.

Run everything from the repository root.

| Tool | Checks | Run |
|---|---|---|
| `fuzz` | the prover against the interpreter on random loop and handler programs (below): the same programs, worlds and statistics as the Python original made for the same seed | `cargo run -p kern-validation --release --bin fuzz -- [options]` |
| `digest` | one hash over every result on a fixed set of modules, the floor for prover changes: S2 (10 modules, full mutation testing, `3cb0e5efa3b63fb8`, unchanged from Python) and `--loops` (loops, handlers, packages, `e7234c3d8ea6a48e` since 2026-11-05; see below). `validation/tests/digest.rs` asserts both | `cargo run -p kern-validation --release --bin digest -- [--loops] [--dump FILE.json]` |
| `difftest` | the compiled Rust program against the interpreter (plus, for the module invariants `kern build` checks, the host's check at the keys a call wrote: a break is a fault): every `eg` line, plus random worlds and calls; A generated crate that doesn't build counts as a failure (`FAILED`, exit 1); only what the emitter doesn't support is skipped (`UNSUPPORTED`) | `cargo run -p kern-validation --release --bin difftest -- [--per-def N] [--seed S] [MODULE.k ...]` |
| `fuzzdiff` | the same comparison on every program `fuzz` generates, template mutants included, all in one crate, each compiled as `kern build` does (module-invariant checks included). Full size, 2026-11-05: 677 programs, 79,898 calls, 0 divergences (150 s); with the checks (f53e1a0): 188 definitions checked, 1,088 calls faulted by a broken invariant, 0 divergences | `cargo run -p kern-validation --release --bin fuzzdiff -- [--seed 1] [--loops 300] [--handlers 200] [--sums 150] [--texts 100] [--no-mutants] [--per-def 100]` |
| `sensitivity` | that `difftest` notices a wrong runtime: plants 18 known bugs in `runtime/` and the emitter (3 in the compiled module-invariant check, 1 in compiled sums, 1 in Text), one at a time, and expects each to fail. The plants go into a copy of the workspace (`--dir`, default `$TMPDIR/kern-sensitivity`, own target and build directories), never into the checkout, because each emitter plant means rebuilding the tool | `cargo run -p kern-validation --release --bin sensitivity -- [--per-def N] [--dir DIR]` |
| `c6` | C6's frozen runner (experiments/challenge/c6/run.py) on the Rust toolchain: same plan and timed region (`analyse` with mutation testing), one child process per configuration; `--emit D K E N STATE` prints a generated module, to compare with `gen.py`. Results: docs/research/CHALLENGE.md, C6 addendum 2026-11-05 | `cargo run -p kern-validation --release --bin c6 -- [--emit D K E N STATE \| --one D K E N STATE]` |

A digest that changes after a prover change is a finding: compare `--dump` outputs before and
after to see which results moved. The digests are a regression floor, not a fault detector: of the
fuzzer's three injected prover bugs, they notice (a) only. The fuzzer finds all three.

**2026-11-05, the loop digest re-baselined.** The Rust toolchain gives each proof a fresh Z3 context
(Fredrik's decision; docs/research/RUST_PORT_PLAN.md). The loop digest went from Python's
`c0e94aa224aef288` to `e7234c3d8ea6a48e`. The dumps differ in one goal only: fz2.k's `f`, goal 2,
which is on Z3's step-limit border. Python gave up on it ("unknown"); Rust finds a counterexample that
doesn't replay, so the goal is `!`. The S2 digest is unchanged.

## `fuzz`: the soundness fuzzer

It looks for proof-page claims the interpreter can show false. It generates random loop and
assumed-handler programs, plus every body mutant of the loop and handler templates (batch.k,
raffle.k, tests/probes). It checks each one (`analyse`, no mutation testing), then runs every
definition on random small worlds. A **contradiction** is a guarantee shown ✔ or ⊙, or a ∅, that
a run breaks. Exit 1 if there is any contradiction or crash.

| option | meaning |
|---|---|
| `--seed N` | the generator seed (default 1); a run is deterministic per seed |
| `--loops N` / `--handlers N` / `--sums N` | how many random programs of each kind (default 300 / 200 / 150). Sum programs (`gen_sum`, since 2026-11-05) come from their own stream, so the loop and handler programs for a seed are unchanged |
| `--texts N` | how many Text programs (`gen_text`, default 100; own stream): set, join with a literal, guard on length, append, compare, and literal joins against the joined literal. The oracle draws Text from a pool with `""`, a backslash, a quote, `é`, `😀` and a literal `\x41` |
| `--variants N` | for the first N sum programs (default 50), a const variant (every literal a `const`) and a record variant (`lo`, `hi` as a `Span` record, passed whole to `agg`). Both are flattened at load, so each must get its base program's verdicts: differences are listed as `variant_mismatches` (reported, not fatal) |
| `--runs N` | interpreter runs per definition (default 300) |
| `--no-mutants` | skip the template mutants |
| `--jobs N` | worker threads (each program runs on a fresh thread) |
| `--fault a\|b\|c` | turn on a known prover bug (the `faults` cargo feature, which only this crate enables, read from `KERN_FAULT`). This shows the fuzzer can find that kind of bug: (a) loop-goal trust propagation off, (b) assumed-handler values taken as 0, (c) the loop exit at HI instead of max(LO, HI) |
| `--save FILE.json` | the summary plus every contradiction or crash, with its program |

Run it after any change to `prove.rs`, `report.rs` or the loop and handler parts of `run.rs`. A
contradiction becomes a `tests/probes/fz*.k` file with a test. `validation/tests/fuzz.rs` runs a small,
deterministic version, and the fault (b) check, with every `cargo test`.

**Results, 2026-11-05, seed 1, full size** (300 loop and 200 handler programs, every template mutant,
300 runs each): 677/677 well-formed, 228,300 runs, 0 contradictions, 0 crashes, 12 s. The faults:
(a) 9,428 contradictions in 66 programs, (b) 15,048 in 92, (c) 3,248 in 26. These are exactly
`fuzz.py`'s FZ2 numbers.

**2026-11-05, sum programs added** (`gen_sum`: sums and state-reading functions, 72d2446). A point update with
before/after sum claims, an accumulating loop whose invariant is a sum, a function read around a write, a filtered
count, and sums against closed forms (`= hi - lo`, a one-element range), correct and then perturbed or negated. A
default run is now 827 programs. Seeds 1-3: 827/827 well-formed, 318,300 runs, 0 contradictions, 0 crashes (24-26 s).
Teeth, by planting bugs in `zz::rec_sum` (not committed): a wrong sign gives 1,314 contradictions in 6 of 150 sum
programs; a wrong empty range (`hi < lo` for `hi <= lo`) gives none, but 223 unconfirmed (`?`) goals against 0, so
`validation/tests/fuzz.rs` also requires 0 unconfirmed on sum programs. Sum proofs are slow: about 1.5 s per program.

**2026-11-05, const and record variants** (`--variants`, default 50). Seeds 1-3: 927/927 well-formed, 378,300
runs, 0 contradictions, 0 crashes, 50/50 const and 50/50 record variants well-formed; 0 const mismatches, 1 record
mismatch per seed (sumr#48 / #22 / #37): the record program's counterexample lacks the entry at a key computed from
a field (`s.lo - 1`), so it doesn't replay and shows `?` where the base shows ✗. Teeth: flattening a record's fields
in reverse order (planted in syntax.rs, not committed) gives 0 contradictions but 9 mismatches in 60.

**2026-11-05, Text programs** (`--texts`, default 100, 135dfca). Teeth, planted in `zz.rs` (not committed): joins
in reverse order give 2,400 contradictions in 8 programs (only the literal-join shape sees it: a consistent
relabelling of `+` passes every claim built with `+`); a length one too long gives 900 in 3. Clean: 0. Found: a
counterexample world leaves Text map entries out, so a refuted Text claim can show `?` (2 per 100 programs).


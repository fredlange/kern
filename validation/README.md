# Validation: checking the toolchain itself

These tools check that Kern's prover, checker and Rust runtime tell the truth. They aren't part of
the distribution (`toolchain/` and `runtime/`), and they aren't unit tests (`tests/`). The Rust ports
live in this directory's crate `kern-validation` (`src/`, one binary per tool). They run longer,
are run by hand after changes to the prover or the runtime, and report numbers into
[docs/research/](../docs/research/). `tests/` runs small, fast versions of some of them.

Run everything from the repository root.

| Tool | Checks | Run |
|---|---|---|
| `fuzz` (Rust; `fuzz.py` is the Python original) | the prover against the interpreter on random loop and handler programs (below): the same programs, worlds and statistics as `fuzz.py` for the same seed | `cargo run -p kern-validation --release --bin fuzz -- [options]` |
| `digest` (Rust; `digest.py` is the Python original) | one hash over every result on a fixed set of modules, the floor for prover changes: S2 (10 modules, full mutation testing, `3cb0e5efa3b63fb8`, unchanged from Python) and `--loops` (loops, handlers, packages, `e7234c3d8ea6a48e` since 2026-11-05; see below). `validation/tests/digest.rs` asserts both | `cargo run -p kern-validation --release --bin digest -- [--loops] [--dump FILE.json]` |
| `difftest` (Rust; `difftest.py` is the Python original) | the compiled Rust program against the interpreter: every `eg` line, plus random worlds and calls; the same scripts as the Python tool (same per-module counts) | `cargo run -p kern-validation --release --bin difftest -- [--per-def N] [--seed S] [MODULE.k ...]` |
| `fuzzdiff.py` | the same comparison on every program `fuzz.py` generates | `.venv/bin/python validation/fuzzdiff.py` |
| `sensitivity.py` | that `difftest.py` notices a wrong runtime: plants known bugs in `runtime/` and the emitter, one at a time, and restores them | `.venv/bin/python validation/sensitivity.py` |
| `oracle_syntax.py` | Rust port, parser: the AST, `code`, hashes and parse errors for every source, as JSON that `toolchain/tests/syntax_oracle.rs` checks | `.venv/bin/python validation/oracle_syntax.py build/oracle_syntax.json` |
| `oracle_run.py` | Rust port, interpreter: outcomes, worlds and invariant checks for every `eg` and random calls, checked by `toolchain/tests/run_oracle.rs` | `.venv/bin/python validation/oracle_run.py build/oracle_run.json` |
| `oracle_analyse.py` | Rust port, everything after parsing: inferred interfaces, guarantees, examples, property tests, mutation, views, `check` output; one module per fresh process; witnesses compared by replay | `.venv/bin/python validation/oracle_analyse.py diff --rust BIN` (`--rust self` checks the harness itself) |
| `pyrandom_vectors.py` | Rust port: CPython 3.9.6's `random.Random` call sequences for `toolchain/tests/pyrandom_parity.rs` | `.venv/bin/python validation/pyrandom_vectors.py` |

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
| `--loops N` / `--handlers N` | how many random programs of each kind (default 300 / 200) |
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
`fuzz.py`'s FZ2 numbers. The Python options are the same (`python -m validation.fuzz`), until the
Python toolchain goes.


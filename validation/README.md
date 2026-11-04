# Validation: checking the toolchain itself

These tools check that Kern's prover, checker and Rust runtime tell the truth. They aren't part of
the distribution (`kern/` and `runtime/`), and they aren't unit tests (`tests/`): they run longer,
are run by hand after changes to the prover or the runtime, and report numbers into
[docs/research/](../docs/research/). `tests/` runs small, fast versions of some of them.

Run everything from the repository root.

| Tool | Checks | Run |
|---|---|---|
| `fuzz.py` | the prover against the interpreter on random loop and handler programs (below) | `.venv/bin/python -m validation.fuzz` |
| `digest.py` | one hash over every result on a fixed set of modules, the floor for prover changes: S2 (10 modules, full mutation testing, `3cb0e5efa3b63fb8`) and `--loops` (loops, handlers, packages, `c0e94aa224aef288`; it was `64051b1d363a6553` before the examples moved, with identical results) | `.venv/bin/python validation/digest.py [--loops] [--dump FILE.json]` |
| `difftest.py` | the compiled Rust program against `kern/run.py`: every `eg` line, plus random worlds and calls | `.venv/bin/python validation/difftest.py [--per-def N] [MODULE.k ...]` |
| `fuzzdiff.py` | the same comparison on every program `fuzz.py` generates | `.venv/bin/python validation/fuzzdiff.py` |
| `sensitivity.py` | that `difftest.py` notices a wrong runtime: plants known bugs in `runtime/` and the emitter, one at a time, and restores them | `.venv/bin/python validation/sensitivity.py` |

A digest that changes after a prover change is a finding: compare `--dump` outputs before and
after to see which results moved.

## `python -m validation.fuzz`: the soundness fuzzer

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
| `--jobs N` | worker processes |
| `--fault a\|b\|c` | inject a known prover bug into a temporary copy of `kern/` and run from there. This shows the fuzzer can find that kind of bug: (a) loop-goal trust propagation off, (b) assumed-handler values taken as 0, (c) the loop exit at HI instead of max(LO, HI) |
| `--save FILE.json` | the summary plus every contradiction or crash, with its program |

Run it after any change to `prove.py`, `report.py` or the loop and handler parts of `run.py`. A
contradiction becomes a `tests/probes/fz*.k` file with a test.


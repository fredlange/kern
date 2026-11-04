# Rust port of the toolchain: plan and pre-registered conditions

Status: 2026-11-04, **draft, not frozen**. Co-owned by the kern-9b and kern-8b sessions. The pass
conditions below are proposals until Fredrik freezes them. After that they're not edited (add a
dated note instead), as in [RUNTIME_PLAN.md](RUNTIME_PLAN.md).

## Decision (Fredrik, 2026-11-04)

"Get rid of Python, use Rust." Kern should install as a developer tool (`cargo install`) or run in
a Docker container. The two sessions work it out together, follow
[FUNDAMENTALS.md](../FUNDAMENTALS.md), and check each other's work.

This replaces part of the runtime plan's decision, which said "the Python toolchain stays,
`kern/run.py` stays the reference semantics forever". Once the port is done, the Rust interpreter
is the reference. **Until then, the Python toolchain is the oracle:** the Rust code is wrong
wherever the two disagree, until shown otherwise.

## Layout (agreed)

- **`toolchain/`:** the Rust toolchain, crate `kern` (a library plus a `kern` binary), so the
  host, the MCP server and the tests share one library.
- **`runtime/`:** kern-rt, unchanged.
- **Root `Cargo.toml`:** a workspace of `[toolchain, runtime]`, excluding `build/` so the
  generated crates don't join it.
- **`kern/`** stays the Python oracle, untouched, until the switch-over. Then the Python is
  deleted and `toolchain/` is moved to `kern/` with `git mv`.
- **Parity harnesses** live in `validation/`, beside `digest.py`, `difftest.py` and `fuzz.py`.

## Split (agreed)

| Who | Modules |
|---|---|
| kern-9b | workspace scaffold, `pyrepr.rs` (Python `repr`, for hashes), `syntax.rs`, `run.rs`, `wf.rs`, `infer.rs`, `views.rs`, `render.rs`, `why.rs`, `baseline.rs` |
| kern-8b | the Z3 binding check, the Python-compatible MT19937, the analyse/views dump and its diff harness, Docker and the install story; after the AST: `prove.rs`, `check.rs`, `report.rs`, `host.rs`, `mcp.rs`, `emit_rust.rs`, `package.rs` |

Each module is one commit, staged by path. Neither session commits the other's files. Before a
module is called done, its author sends the other session its diff-harness result.

## Crate-wide rules (agreed)

- **No `std` `HashMap`/`HashSet` where iteration order can reach terms, mutants or output.** Use
  `IndexMap` (Python dict order) or `BTreeMap`. This is how FZ finding 6 doesn't come back.
- **Hashes are SHA-256 over Python's `repr`** of the AST tuples, truncated to 8 hex digits. That
  covers `Def.hash`, `content_hash`, the `why` interface anchor and the report cache key.
  `pyrepr.rs` reproduces `repr` byte for byte: string quoting and escapes, `(a,)`, lists, ints,
  `True`/`False`/`None`.
- **`/` and `%` are floor division and floor modulo,** as in Python and kern-rt. Rust's `/`
  truncates.
- **Randomness is MT19937, compatible with Python 3.9.6 (the venv's version).** That covers
  integer and string seeds (string seeds go through SHA-512), `random()`, `randint`/`randrange`
  through `_randbelow` with rejection, `choice` and `shuffle`. Without it, property-test and
  mutation numbers don't match, and neither do assumed-handler values, the host's SimNet or its
  audit sampling.
- **Z3 is the same version as Python's:** `z3-solver` bundles libz3 5.3.3, and Homebrew has z3
  5.3.3. Solver limits stay as `rlimit` step counts (`RLIMIT = 10_000_000`).

## Pass conditions (proposed; Fredrik freezes them)

| # | Condition | How it's measured |
|---|---|---|
| R0 | `Def.hash` and `content_hash` are identical for every definition in `examples/` and `tests/probes/` | harness tier 0 (syntax only) |
| R1 | Per module, on every example and probe: verdicts, trust tiers, claim text and mutant kill counts are byte-identical to Python's. `eg` witnesses are compared by outcome: a Rust counterexample must replay to the same outcome in the interpreter, but its values may differ | the diff harness |
| R2 | Both digests come out unchanged from the Rust toolchain: S2 `3cb0e5efa3b63fb8` and loops `c0e94aa224aef288` | `validation/digest.py` against the binary |
| R3 | The soundness fuzzer finds 0 contradictions and every injected fault | `validation/fuzz.py` (or its port) |
| R4 | The test suite passes against the Rust toolchain | pytest driving the binary, or ported cargo tests (open question O3) |
| R5 | `kern check` is no slower than Python on any example, cold and with the cache | timed |
| R6 | `cargo install --path toolchain` works on a clean macOS machine, and the Docker image runs the diff harness green **inside** the container (not just "the binary starts") | a fresh machine or a clean container |

**Fallback for R1 and R2, proposed:** the same libz3 version doesn't promise the same witnesses.
A borderline result could flip between ✗ (counterexample confirmed) and ? (unconfirmed) if a
Rust witness doesn't replay where Python's did. Each such flip is listed with both witnesses, and
the digest is re-baselined with a dated note (as was done for the loop digest). Any other change
in a verdict is a bug in the port.

## Open decisions for Fredrik

**Decided by Fredrik, 2026-11-04** (asked by kern-9b; the options as put to him are below):
- **O1:** `i128` with checked arithmetic (overflow is an atomic error), the alias the port already
  uses. The runtime's `i64` stays a documented divergence. No new prover obligation.
- **O2:** everything except `experiments/`. `kern/`, `tests/` and `validation/` move to Rust;
  `experiments/` stay as frozen Python records.
- **O3:** tests are ported per module, to cargo tests, along with the Python-oracle diff. pytest
  goes at cutover.
- **O5:** Text is built in the Rust toolchain once the modules it touches reach parity. Python
  stays frozen (bug fixes only) until then.
- O4 is still open (technical: kern-8b is testing the bundled z3 build in Docker).

- **O1. Int semantics.** `run.py` has unbounded integers, and the prover reasons about unbounded
  integers. kern-rt uses `i64`, where overflow is a fault: atomic, rolled back, no clock tick.
  That's a documented divergence. kern-9b proposes `i64` with checked overflow in the Rust
  interpreter too, to match the runtime. That makes `i64` the language's meaning, which is a
  language change, not a port detail.
  - *What it changes:* guarantees still hold on success, because a fault is atomic. But an
    `ok otherwise` claim ("succeeds whenever its needs hold") could be false for values near
    2^63, and the prover wouldn't see it.
  - *The alternatives:* (a) `i64` plus a new prover obligation that arithmetic stays in range,
    or a documented exception to `ok otherwise`; (b) `BigInt` in the toolchain interpreter, which
    keeps today's meaning, while the runtime's `i64` stays a documented divergence.
  - Until this is decided, the int type sits behind one alias in the Rust code. For now it's `pub type Int = i128` with checked
    arithmetic (toolchain/src/lib.rs): the closest to Python without a BigInt dependency.
- **O2. Scope of "no Python".**
  - (a) The distribution only (`kern/`). `tests/` and `validation/` stay in Python, driving the
    binary.
  - (b) Everything except the frozen experiments, which are records and can't be ported.
  - (c) Everything.
- **O3. Tests during the port.** Most tests import the Python API, not the CLI. Either keep
  pytest and add a JSON-over-stdin mode to the binary so the tests can call it, or port each
  test module to cargo tests along with its module.
- **O4. Z3 in Docker.** Distribution `libz3-dev` packages aren't 5.3.3. Either build 5.3.3 from
  source in a builder stage, or use the `z3` crate's static/bundled build, pinned to 5.3.3 if it
  can be.

- **O5. Text (kern-44's request R1).** The random-portal app needs text from a JSON reply, stored
  in state and readable by clients, with `len` in specs. That's a language change touching syntax,
  wf, infer, run, prove, the host and the proof page. The sessions propose freezing Python
  semantics during the port (bug fixes only, so the oracles stay valid) and adding Text to the
  Rust toolchain only, once those modules match. The decision is whether it comes during the
  port or after the switch-over.

## Results

All 2026-11-04, measured on this machine.

**Z3 binding (kern-8b): go.**
- **The crate:** `z3` 0.22.0 (z3-sys 0.14.1). Against Homebrew it links
  `/opt/homebrew/opt/z3/lib/libz3.5.3.dylib` (5.3.3), found through pkg-config.
- **Coverage:** a probe exercised every Z3 feature `prove.py` and `report.py` use: Int and
  Array select/store, `forall_const`/`exists_const`, implies/ite, uninterpreted `FuncDecl`,
  `distinct`, Solver and Optimize with `rlimit` params, Optimize minimize, model eval with
  completion, the array-model walk (Store chains, as-array, func-interp entries), and
  `reason_unknown` after the limit trips.
- **Parity:** on the same six problems, the output was byte-identical to Python z3's, witness
  values included.
- **The `bundled` feature** builds z3-src 503.3.0, which is Z3 5.3.3.0, so the version matches
  for a static Docker build. It needs cmake, which isn't installed on this Mac. To be tested in
  the Docker builder.

**MT19937 (kern-8b): done.** `toolchain/src/pyrandom.rs` (5b77093) replays 6,000 interleaved
calls over 15 seeds against CPython 3.9.6 with 0 mismatches. The seeds include int, negative,
over 64 bits, and str seeds, with unicode and empty strings among them. The vectors come from
`validation/pyrandom_vectors.py`.

**syntax.rs and pyrepr.rs (kern-9b, 0f19d91): R0 holds.** 0 mismatches over 583 sources (1,393
definitions, 170 parse errors), comparing fields, `code`, `stmt_code`, `hash`, `content_hash` and
error text. Planted faults were caught. The places where Python crashes with an uncaught
exception are recorded as divergences.

**run.rs (kern-9b, 267d348):** 0 mismatches over 97,310 interpreter calls in 390 modules
(`validation/oracle_run.py`). Three planted changes were all caught.

**The analyse oracle (kern-8b):** `validation/oracle_analyse.py`, run as `diff --rust BIN`, over
94 modules: every example (packages as units), the probes, the docs' kern blocks, chain modules
and 60 fuzz programs. Its self-diff (Python against Python, in another process with another hash
seed) agrees on 94/94. Building it found two things the comparison has to respect:
- **Dumps run one module per fresh process.** Within one process, Z3's witness choice depends on
  earlier solver calls (a known limitation). With every module in one process, fuzz_32's witness
  differed by corpus position.
- **An eg line doesn't record assumed-handler draws,** so a witness in a module with handlers
  can't be replayed. There the witness has to be identical, not just replay-equivalent.

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
| `digest` | one hash over every result on a fixed set of modules, the floor for prover changes: S2 (10 modules, full mutation testing, `3cb0e5efa3b63fb8`, unchanged from Python) and `--loops` (loops, handlers, packages, `51c881e8e3cd2e9b` since 4b44b2f; see below and validation/tests/digest.rs). `validation/tests/digest.rs` asserts both | `cargo run -p kern-validation --release --bin digest -- [--loops] [--dump FILE.json]` |
| `difftest` | the compiled Rust program against the interpreter (plus, for the module invariants `kern build` checks, the host's check at the keys a call wrote: a break is a fault): every `eg` line, plus random worlds and calls; A generated crate that doesn't build counts as a failure (`FAILED`, exit 1); only what the emitter doesn't support is skipped (`UNSUPPORTED`) | `cargo run -p kern-validation --release --bin difftest -- [--per-def N] [--seed S] [MODULE.k ...]` |
| `fuzzdiff` | the same comparison on every program `fuzz` generates, template mutants included, all in one crate, each compiled as `kern build` does (module-invariant checks included). Full size, 2026-11-05: 677 programs, 79,898 calls, 0 divergences (150 s); with the checks (f53e1a0): 188 definitions checked, 1,088 calls faulted by a broken invariant, 0 divergences | `cargo run -p kern-validation --release --bin fuzzdiff -- [--seed 1] [--loops 300] [--handlers 200] [--sums 150] [--texts 100] [--no-mutants] [--per-def 100]` (`--texts N` also sets the number of nest and match programs) |
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
| `--nests N` / `--matches N` | nested-loop and variable × variable programs (`gen_nest`: a nested count with `(r - lo) * n` invariants, grid writes at `r * W + c` with frame claims, nonlinear identities) and `match` programs (`gen_match`: Int, constant, negative and Text patterns, with or without `_`); default 100 each, own streams |
| `--ticks N` | `clock.tick` programs (`gen_tick`, default 100, own stream): n ticks then a store and an event with an `eg` that pins it, a ticking callee, ticks in a loop |
| `--frames N` | stated loop frames (`gen_frame`, default 100, own stream): a loop writing `hold` at `k + OFF`, with `keep hold except A to B` right or one key too narrow, and `keep other` |
| `--ckeys N` | computed-key writes under module invariants (`gen_ckey`, default 100, own stream): `k * C + D` or a key read from a map, a value that is non-negative, guarded, copied or possibly negative, one invariant or two across maps |
| `--recs N` | recursion (`gen_rec`, default 100, own stream): a counting-down function, a mutual pair, a recursive procedure; the recursive argument decreases or not, claims right or near misses. A depth-limit error contradicts the page only if every measure the call can reach is proven |
| `--lits N` | literal-range claims (`gen_lit`, default 100, own stream): `for j from A to A + W`, W 1..17, sometimes `when`, over a body writing every key, all but the first or last, or one more |
| `--rets N` | returning procedures (`gen_ret`, default 100, own stream): an allocator `alloc -> Int` (spec exact or loose), its id used as a key, two calls chained, calls in a loop, a returning procedure that refuses; claims right, off by one, or true only if the value were 0 |
| `--accts N` | Accounts as state values (`gen_acct`, default 100, own stream): join, pay through a stored member with the `!= none` guard sometimes dropped, move, leave, a boss chain. The oracle draws Account-valued entries from the world's accounts and `none`; a 'none is not an account' run contradicts the page only if every "is an account, not none" goal of the definition is proven |
| `--variants N` | for the first N sum programs (default 50), a const variant (every literal a `const`), a record variant (`lo`, `hi` as a `Span` record, passed whole to `agg`) a package variant (`agg` in a package `lib` the program's package `use`s), a nested-record variant (`Span` inside `record Wrap s:Span tag:Int`, `w.s` passed whole) and a record-state variant (`hold`/`flag` as the fields of `state cell Int -> Cell`). Both are flattened at load, so each must get its base program's verdicts: differences are listed as `variant_mismatches` (fatal since 7d011af fixed the record counterexamples) |
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

**2026-11-05, nested loops, variable × variable and `match`** (59c9e66, a8cee99; `--nests`, `--matches`). Teeth: an
outer loop frame that ignores the inner loop's writes (planted in prove.rs's `loop_effects`, not committed) gives
3,827 contradictions in 19 of 100 nest programs. True grid frame claims stay `?` (the outer frame forgets inner
writes by design). `validation/tests/fuzz.rs` also checks that 300 generated matches parse to exactly the
definitions of their hand-written `if` chains (those with a `-1` pattern skipped: no expression spells the literal -1).

**2026-11-05, `clock.tick` and imports** (e1e1a68, b71848b; `--ticks`, package variants). Teeth, planted in prove.rs
(not committed): a tick of +2 gives 1,423 contradictions in 10 of 100 tick programs; a ticking callee taken as not
moving the time gives 6,000 in 20 (only claims that hold without a tick see it). Package variants: 0 mismatches.

**2026-11-05, stated loop frames** (48dd716; `--frames`). Teeth: the frame's `j >= B` planted as `j > B` (prove.rs,
not committed) gives 3,103 contradictions in 24 of 100 frame programs. Clean: 90 frames proven, 62 refuted.

**2026-11-05, computed-key invariants** (4b44b2f; `--ckeys`). Teeth: the goal's after-state side evaluated on the
pre-state (prove.rs, not committed) gives 5,688 contradictions in 35 of 100. Clean: 74 proven, 27 refuted, 43 `?`
(a false invariant whose counterexample doesn't replay; random tests often break it too, so ✗ would be earned).

**2026-11-05, recursion** (3700f8d, 6eb422f; `--recs`). Teeth: the measure goal's `<` planted as `<=` (prove.rs, not
committed) gives 3,060 contradictions in 14 of 100 (each a depth-limit error under a measure shown proven). Clean:
0, with depth-limit errors under an unproven measure counted as skipped.

**2026-11-05, literal ranges** (14e8e67; `--lits`). Teeth: the expansion planted to drop the last instance
(prove.rs, not committed) gives 7,165 contradictions in 26 of 100. Clean: 47 proven, 53 refuted.

**2026-11-06, returning procedures** (700e189; `--rets`). Teeth: a modular call handing the caller 0 instead of the
result the callee's ensures constrain (prove.rs, not committed) gives 5,350 contradictions in 21 of 100; it needed
claims that hold only if the value were 0 (a claim comparing the value with the callee's own formula only fails).

**2026-11-06, nested records and records in state** (00fedb4, 6506a42). Two more variants per sum program, compared
verdict for verdict: 0 mismatches. A reversed leaf order for nested records (planted in syntax.rs) changes no verdict,
since fields are matched by name; it changes positional arguments, which `validation/tests/fuzz.rs`
`a_nested_record_argument_is_positional_in_declaration_order` pins with an `eg` (the plant fails it). Record equality
and whole-record sets are checked against their field expansions (same AST). Not yet: a record-state variant of the
frame programs, since a frame on one field (`keep cell.hold except …`) isn't accepted (sent to lang). (Accepted since d3bc291: the frame programs' record-state variants run, 0 mismatches.)
Returning procedures get a record-returning variant (`alloc -> Slot`, callers bind it and read `.id`, 1bb9cef): 0
mismatches. Writing it found a type hole (an Int assigned to a `-> Point` result passes wf and panics the prover).

**2026-11-06, Accounts as state values** (4a9d41d; `--accts`). Teeth: the "is an account, not none" goal planted as
always true (prove.rs, not committed) gives 1,622 contradictions in 20 of 100. Clean: 0. Open (sent to lang): a
definition whose none goal is refuted still shows `✔ ok otherwise`; the oracle doesn't count it until that is settled.


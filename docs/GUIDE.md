# How to Kern

A working guide: set up, write a module, read what `check` says, fix it, keep it honest, run it.
The language itself is in [REFERENCE.md](REFERENCE.md); read that first. This guide covers
the tools, the file formats and the Rust library API.

`kern/tests/docs.rs` keeps this file honest. Every `kern` code block must parse and
check. Every CLI command and option must be documented here, and every API name listed must
exist. If you change any of them, update this file in the same commit (see
[AGENTS.md](../AGENTS.md)).

## Set up

Kern is one binary, `kern`, built from `kern/` (Rust). Install it as a dev tool:

```text
brew install z3                              # Z3 5.3.3, the version the prover is pinned to
cargo install --path kern                    # puts `kern` in ~/.cargo/bin
kern check examples/ledger-tour/ledger_v3.k
```

Without a system Z3, `cargo install --path kern --features static-z3` compiles Z3 5.3.3 from
source and links it in (needs `cmake` and a C++ compiler; a few minutes). Or use the container,
which needs nothing but Docker:

```text
docker build -t kern .
docker run --rm -v "$PWD:/work" kern check examples/shop --quick
docker run --rm -i -v "$PWD:/work" kern mcp                      # the MCP server on stdio
docker run --rm -p 8080:8080 -v "$PWD:/work" kern host serve examples/shop --bind 0.0.0.0
```

- **Token counts** (`view`, the MCP tools) need the Claude tokenizer file that `anthropic==0.34.2`
  bundles (`anthropic/tokenizer.json` inside that wheel on PyPI): set `KERN_TOKENIZER=/path/tokenizer.json`,
  or put it in `~/.cache/kern/tokenizer.json`. Without it, counts are simply absent.
- **`kern build`** needs `cargo`. An installed `kern` carries the `kern-rt` sources and writes them
  out under the build directory; `KERN_RUNTIME` and `KERN_BUILD` override where the runtime and the
  build directory are.
- **Mutation testing** scores mutants on `KERN_JOBS` threads (default 4, at most the core count; `1` is
  serial). Results are the same for any value. More than about 4 threads gets slower, not faster, because
  Z3 builds every solver under one global lock.
- **Tests:** `cargo test --workspace --release` from the repository root. `kern/tests/golden/` holds the
  recorded output of every example and probe; after an intended change, re-record with
  `KERN_BLESS=1 cargo test -p kern --release --test golden` and say why in the commit. Validation tools: [validation/README.md](../validation/README.md);
  experiments: [experiments/README.md](../experiments/README.md).

## Anatomy of a definition

```text
transfer from:Account to:Account amount:Int        header
  need amount > 0 else BadAmount                    refuses with BadAmount
  need ledger.balance from >= amount else Insufficient (amount - ledger.balance from)
  keep ledger.total                                  same value before and after
  ensure ledger.balance' to = ledger.balance to + amount   ' = after the call
  emit TransferDone from to amount clock.now         exactly these audit events (_ = any value)
  ok otherwise                                       optional: succeeds whenever the needs hold
  ensure ledger.balance' to = ledger.balance to when amount = 0   a clause can hold only `when` C
  eg A:500@u1 B:0@u1 | transfer A B 100 -> ok | A:400 B:100   executed example
body
  ledger.debit from amount                          a body may have `need`s too; under
                                                    `ok otherwise` they must never fire
```

- **`uses` (effects) and `fails` (failure set) are inferred.** You never write them.
- **`emit` lists the exact sequence of events** the call appends to the audit log. With no `emit`
  clause the log is unconstrained. It is not inferred from the body: a spec projected from the
  body changes when the body changes, so it could never catch a bug.
- **A refusal undoes the whole call,** so guarantees only have to hold on success.
- **`why` entries live in a separate file** (see Files), anchored to the definition's hash *and*
  its inferred interface. They go stale when the code changes, or when a callee change alters
  what this definition can do or refuse. Nothing correctness-critical may live there: if it must
  be obeyed, it is a `need`, `keep` or `ensure`.

## What `check` does

| Step | Method |
|---|---|
| Well-formedness, types | names, arity, no shadowing, pure specs, no recursion; Int / Account / Owner / Bool / Event / Unit. Nothing ill-formed is proven or run |
| Effects, failures | inferred from the body, transitively through callees |
| Writes, refusal conditions | inferred: which balances and state can change (the frame modular proofs use), and the body-level conditions under which a call refuses |
| Examples | run in the interpreter; each failing one gets a `failing: eg …   (got …)` line: the outcome, or the error (wrong argument count, unknown account), or, when only the after-state differs, the listed items' actual values |
| Random tests | 500 random worlds and inputs; every guarantee is checked on each success |
| Proofs | symbolic execution to Z3. Calls are verified *modularly*, from the callee's spec and inferred interface, never its body |
| Refusals | with `ok otherwise`, proven to refuse only where a spec `need` fails |
| Tiers | ✔ proven, ✗ refuted (replayed), ? unconfirmed or no answer, ! relies on something unproven, ⊙ relies on an assumed handler, ◐ tested only, ○ a module invariant checked only at run time, after every call (`kern host` and `kern build` programs; the gated host serves it only with `--no-gate`), ∅ never succeeds |
| Trust | a modular proof is downgraded if a callee's guarantees are broken |
| Counterexamples | rendered as `eg` lines and replayed in the interpreter; only a replayed one counts as ✗ |
| Spec strength | mutation testing of the body. The headline is what the *guarantees* catch; examples are reported separately |
| Spec refusals | each spec `need` dropped, and its comparison flipped; caught by an example, else a guarantee (cheapest first). `unpinned` lists the needs nothing pins; an `eg` at the boundary fixes it. Callers' proofs aren't re-run |
| Ratchet | `kern baseline FILE.k` records strength and guarantees; `check` flags "Weaker than baseline" |
| Proof page | fixed templates over the results. No model-written text |

## Agent views

`kern view FILE.k NAME --as VIEW` (and the MCP tool `kern_view`) shows what an agent reads:

| View | Contents | `payout` in `ledger_v3.k`, measured |
|---|---|---|
| `iface` | header, uses, writes, fails, why stub | 45 tokens |
| `spec` | + need, keep, ensure, emit, own `body need` lines, proof status | 77 tokens |
| `editor` | own full definition + the `spec` view of each callee (never callee bodies) | 251 tokens |

Recursion needs a measure: `decreases n` on each definition of the cycle. The prover checks, at every recursive
call, that the callee's measure at the arguments is ≥ 0 and smaller than the caller's (a `measure` goal); the call
itself is proven from the callee's spec, as any call is. The interpreter refuses calls nested deeper than 200 (a
Kern error; random-test runs that reach it are skipped).

An `eg` line may end with a fourth section, the events: `| A:400 B:100 | Moved A B 100 0` (exact on `audit`, and on
any channel it names, `console Line "hi"`; an empty section means nothing on `audit`). A failing one prints what
was appended: `failing: eg …   (got ok, appending Moved A B 100 7)`. `clock.tick` is a statement: one call can then
see two instants (`clock.now` reads the current one; specs read the start).

Text (`t:Text`, `state title Int -> Text`, `"…"`, `length`, `+`) is proven with Z3's strings. In `eg` lines,
counterexamples and `kern run` a Text is quoted (`name_it 1 "a b"`, `title.1:"a b"`), so a literal may hold
spaces and `|`. Random tests draw Text from a small pool (`""`, `"a"`, `"ab"`, `"hello world"`, `"Ω"`).
`kern build` and the host's store don't carry Text yet (runtime's follow-up).

A sum (`sum j from A to B of E`) is proven through a Z3 recursive function, so a loop invariant like
`ledger.balance' pool = ledger.balance pool + sum j from lo to k of hold.get j` is kept by one unfolding per
step. Each application of a sum also gets its bounds: if every term lies in [m, M] whatever the state (interval
arithmetic on the term, or a Z3 check of `term >= 0` / `term <= 0`, filter included), the sum lies in
[m·n, M·n] with n = max(0, B − A). So a count is between 0 and B − A, and a sum of non-negative terms is
non-negative, without induction. After a write, a body `need` that sums over state isn't lifted into the pre-state (no `body need` line).

A record type in the header adds a line `  record Name f:Type …` to every view (a caller needs the fields to build
one). The checker flattens a record parameter `p:Point` into `p.x`, `p.y` at load time, so `eg` lines,
counterexamples, mutants and the host take its fields as separate arguments, in order.

A `const NAME N` the shown source mentions adds a line `  const NAME N` after the interface lines. The checker
substitutes the value at load time, so lines printed from the checked code (`check` output, `body need` lines,
expanded `need as`, counterexamples, the proof page) show the value, and changing a constant's value changes the
build hash of every definition that uses it: their `.why` entries go stale and their `.kern-cache` entries miss.

## The loop

1. **Write the spec before the body.** Write the `need`s, then `ok otherwise` if callers may
   rely on *when* the call succeeds, then `keep`/`ensure`/`emit`, and one or two `eg` lines.
2. **Check:** `kern check FILE.k`. Everything must read `All guarantees proven`.
   While iterating, use `check --quick` (no mutation testing). Definitions whose code, and
   everything they reach, didn't change come from `.kern-cache/` (shown on stderr). Run a
   full `check` before you commit, since strength is only measured there.
3. **Read a ✗ as a test case.** It comes with an `eg` line that was replayed in the
   interpreter, for example:

   ```text
   ✗ ok otherwise
       counterexample: eg A:1@u1 B:253@u1 | payout A B B 1 0 -> BadAmount | A:1 B:253
   ```

   Here a call that passes payout's needs still refuses (transfer refuses a zero fee). Either
   the body is wrong, or the spec is: in this case add the missing `need`. Paste the line
   into the spec as an `eg` if it's worth keeping.
4. **Read a `?`, `!`, ◐ or ○ as a spec too weak to prove from.**
   - ○ "writes a computed key": the invariant is checked key by key at the parameters, so write state
     at a parameter key, or accept the host's runtime check.
   - `?` with "not reproduced": a callee's spec doesn't say enough, so strengthen the
     callee's `ensure`.
   - `!`: a callee isn't proven, so fix the callee first.
   - "relies on X, whose spec does not say what it emits / when it refuses": give X an
     `emit` or an `ok otherwise`.
5. **Read `missed:` lines.** Each is a body change no guarantee would notice. Add the
   guarantee that pins it, or accept it if the mutant is equivalent (it changes nothing).
6. **Ratchet:** `kern baseline FILE.k` records, in `FILE.strength`, the
   guarantee strength, the guarantee clauses, the spec `need`s and each definition's own
   body-level refusals.
   - From then on, `check` flags "Weaker than baseline" (a guarantee removed, strength
     fell) and "Spec changed since baseline" (a refusal added, removed or changed).
   - Commit the baseline and use `check --against REV`, so the edit you're checking can't
     also rewrite the baseline.

**Don't** add a `need` that refuses the case you can't prove, delete a guarantee to go green,
or re-run `kern baseline` to hide a drop. Each of these changes what callers may rely on;
say so instead.

## Packages: several files

A program of several files is a **package**: a directory with a `kern.pkg` file (empty is fine).

```text
shop/
  kern.pkg
  money/ledger.k          module money.ledger: transfer
  inventory/catalog.k     module inventory.catalog: state price, stock; set_price, restock
  orders.k                import money.ledger / import inventory.catalog; buy, refund
```

- **`import a.b`** at column 0 names a module by its path from the root. Names come in
  unqualified, and a file may use only what it declares or imports. That covers definitions,
  `state`, `handler` and `channel`. Leaving an import out is an error that says which one to
  add.
- **Names are unique across the package.** A duplicate is an error naming both files.
- **Every tool loads the whole package from any of its files:** `check`, `view`, the MCP tools
  and the host. `check` prints a `== module (file)` line before each file's definitions, and the
  page says which module each definition is in.
- **MCP edits** go to the file you pass. Editing a definition that lives in another file is
  refused, with its file named, and every caller across the package is re-checked.
- **Outside a package,** a file with `import` lines loads itself plus what it imports, resolved
  from its own directory.
- **The prover, interpreter and host** see one merged module, so specs, proofs and trust work
  across files exactly as within one.

Examples: `examples/shop/`, `examples/library/`, `examples/lottery/` (each with a README).

## `kern` commands

| command | does |
|---|---|
| `check FILE.k` | prove, run the examples and random tests, measure spec strength; exit 1 if anything is not clean. In a package (or a package directory), every file of the package, grouped by module |
| `view FILE.k NAME --as iface\|spec\|editor` | what an agent reads (token count on stderr). `editor` = the full definition plus its callees' spec views |
| `why FILE.k NAME` | the rationale entries for NAME, with staleness |
| `anchor FILE.k` | re-anchor the `.why` file to the current builds (confirm the entries still hold first) |
| `run FILE.k "NAME ARGS" --world "A:500@u1 …"` | run one call in the interpreter |
| `baseline FILE.k` | record the ratchet baseline (`FILE.strength`) |
| `build FILE.k` | compile to a native binary (see Compiling to Rust); prints the binary's path. Needs `cargo` |

| option | for |
|---|---|
| `--why FILE.why` | the rationale store (default `FILE.why` next to the module; `/dev/null` for none) |
| `--html OUT.html` | `check`: also write the proof page |
| `--as iface\|spec\|editor` | `view`: which view |
| `--world WORLD` | `run`: the world, in `eg` syntax |
| `--baseline FILE.strength` | `check`: compare against this baseline (default `FILE.strength`) |
| `--quick` | `check`: no mutation testing (most of the time); strength reads "not measured" unless the cache has it for exactly this code |
| `--cache DIR` | `check`: reuse results of definitions whose code (and everything they reach) is unchanged (default `.kern-cache`, `''` = off) |
| `--out DIR` | `build`: where to write the Rust crate (default `build/<module>`) |
| `--against REV` | `check`: take the baseline from git at REV (`git show REV:FILE.strength`), so rewriting the file can't hide a change |

## `kern host` commands: run a module for real

| command | does |
|---|---|
| `manifest FILE.k` | the effects and network endpoints the module can reach, and which definitions are served |
| `repl FILE.k --db X.db --world "…"` | one call per line on stdin; events are delivered after commit |
| `serve FILE.k --db X.db --port 8080` | HTTP: `POST /NAME` with a JSON object of the parameters (a record parameter as one object of its fields, `{"p": {"x": 1, "y": 2}}`), `GET /state`, `GET /manifest` |
| `replay FILE.k --db X.db` | re-run the call log from genesis; it must match the database |
| `page FILE.k --html OUT.html` | the proof page plus a Host section (everything marked *assumed*) |

| option | for |
|---|---|
| `--db X.db` | the SQLite database (default in memory) |
| `--world WORLD` | the genesis world (only on a new database; `serve` and `repl` start a new database with an empty world when it's left out) |
| `--port N` | `serve` |
| `--bind ADDR` | `serve`: the address to listen on (default `127.0.0.1`; `0.0.0.0` inside a container) |
| `--no-gate` | also serve definitions that aren't proven clean (the invariants are still checked at runtime) |
| `--config FILE.host` | the network endpoints (default `FILE.host` next to the module) |
| `--skip-proven` | skip the per-call invariant check for definitions whose invariant goals are all proven ✔ (the manifest says which). Off by default: the check is on for every definition until the loop and handler fuzzer has run clean (decided 2026-11-04) |
| `--audit-rate R` | with `--skip-proven`: still run the skipped checks on a fraction R of calls (defence in depth; the counts are in the manifest). The full invariant audit at startup always runs |
| `--html OUT.html` | `page` |
| `--why FILE.why` | `page`: the rationale store (default `FILE.why` next to the module, as `kern check`) |

- **One call is one transaction.** A refusal writes nothing, and an unproven definition isn't
  served. HTTP status codes: ok → 200, refused → 409, bad call → 400, unproven → 403, broken
  invariant, an Int outside i64 or more than 10,000,000 loop and sum iterations in one call → 500 (docs/HOST.md).
- **Delivery is at-least-once,** so consumers dedupe on `seq`.
- **Assumed handlers** (`handler rand.below …`):
  - the host must provide each one, or it refuses to start;
  - every value is checked against the assumed spec, and a violation is an atomic error;
  - every value is recorded in `log.observed`, so `replay` feeds the same values back.
- **Network replies arrive as new calls.** `serve` and `repl` send a call's requests after it commits, then run
  each reply (or failure) as its own logged call; `repl` prints them as `reply NAME ARGS -> ok`. Details are in
  docs/HOST.md.

## The MCP server: working through views

`kern mcp` serves the tools below over stdio (MCP, JSON-RPC 2.0), so an agent works through views
instead of files. To register it with Claude Code: `claude mcp add kern -- kern mcp`, or pass the
same command with `--mcp-config`.

| tool | does |
|---|---|
| `kern_list` | the definitions: interface line and status |
| `kern_view` | `iface`, `spec` or `editor` view of one definition (as `kern view`, except for repeated callee specs, below); `full: true` sends every block |
| `kern_callers` | who reaches a definition, so you know whose guarantees an edit can break |
| `kern_why` | the rationale entries, with staleness |
| `kern_check` | a summary (status per definition plus what's broken); `name` gives full detail; `quick` (default) skips mutation testing |
| `kern_run` | one call in the interpreter |
| `kern_edit` | several `add` / `replace` / `remove` operations, checked once; if the result is ill-formed, nothing is written |
| `kern_add`, `kern_replace`, `kern_remove` | one structural edit. `expect_hash` refuses if the definition changed underneath. `kern_add` also takes one declaration line (`const`, `record`, `state`, `channel`, `invariant`) |

What an edit does and reports:
- **It's checked before anything is written.** If the result is ill-formed, the file stays
  unchanged; syntax errors count lines from the submitted definition.
- **Otherwise the file is written,** and the edited definitions and all their callers are
  re-checked.
- **The result** has `ok`, `broken` (with the ✗ lines), `rechecked`, and the `why` entries the
  edit made stale or orphaned.
- **A red result is still written:** a change across callee and callers is red in between, so
  fix forward.
- **`kern_remove` refuses** while anything references the name.
- **`KERN_MCP_LOG=path`** logs one JSON line per call, with the tokens it returned.

One server process is one session. It remembers the latest thing it showed about each definition. A later
editor view sends a callee's spec block as one line when what the session last showed of that callee still
holds:
- the same block, word for word (sent as a callee block or a `spec` view): `move: spec shown earlier in this session`;
- an editor view of the callee whose lines before `body`, without `eg` lines, are the block without its proof line:
  the stub carries the proof line, `move: spec shown earlier in this session; proof ok`.

`full: true` sends every block, for an agent that lost its context. The test is on text, not on the definition's
hash, so a callee whose inferred `fails` or proof line changed (a callee of it was edited) is sent again, and so
is one whose own edit removed a line. A block with `body need` or expanded `need as` lines is always sent.
`kern_view`'s description asks agents to read with `spec` and use `editor` only for a definition they will change. C2's K-view agents used `editor` for all
35 views, and repeated callee specs were 27–34% of what they read
([experiments/challenge/c2/PROTOCOL.md](../experiments/challenge/c2/PROTOCOL.md), 2026-11-05).

## Compiling to Rust

`kern build FILE.k` writes a Rust crate for the module (or the whole package) to `build/<module>/`,
links it with the runtime crate `kern-rt` in `runtime/`, and runs `cargo build --release` (the
target directory is `build/target`). `need`s compile to code. `keep`, `ensure` and
`emit` are proven, so they compile to nothing. `kern build` runs the prover first (no mutation testing): a module
invariant that isn't ✔ for a definition is compiled as a check after that definition's calls, at the keys the call
wrote, and a broken one undoes the call as a fault (docs/HOST.md, key-scoped invariant check). The interpreter (`kern/src/run.rs`) stays the
reference semantics: the runtime is wrong wherever the two disagree. The contract and the known divergences (for example,
Int is i64 at run time, i128 in the interpreter and the prover's model is unbounded) are in
[research/RUNTIME_PLAN.md](research/RUNTIME_PLAN.md). Needs `cargo`.

## Files

- **`FILE.k`:** the module.
- **`kern.pkg`:** marks a package root (it may be empty). A line `use ../money` brings in a sibling package: its
  modules are imported as `money.<module>`, and checking this package checks them too. In a package, the `.why` and `.strength`
  files are per package: `<root>/kern.why` and `<root>/kern.strength`.
- **`FILE.why`:** rationale, never obligations. Each block is a definition name and its
  *interface anchor*, followed by indented entries:

  ```text
  transfer@4be917d8
    Insufficient carries the shortfall so support can tell the customer how much is missing
  ```

  The anchor hashes the build hash together with the inferred interface (uses, fails, writes,
  lifted refusals), so it is not the build hash `check` prints. Write any placeholder
  (`transfer@new`) and run `kern anchor FILE.k`; `kern why` shows both anchors when they differ.
  An entry goes stale when the definition or its inferred interface changes. A line at column 0
  that isn't `name@anchor` is an error. An entry that reads like a rule ("must …") is flagged:
  move it into the spec.
- **`FILE.strength`:** the ratchet baseline, written by `kern baseline`. Don't edit it by hand.
- **`FILE.host`** (in a package, `<root>/kern.host`): one line per network request constructor:

  ```text
  Fetch -> GET http://127.0.0.1:9/limit/{1}; reply got_limit {1} {2} $.value; fail limit_failed {1} {2}; timeout 300
  ```

  `{i}` is the event's i-th argument and `$.field` an Int from the JSON reply (`reply` only: a failure has no
  body). `timeout N` is optional: a request that no ok call to its reply or fail definition has answered within N
  seconds of the host clock (default 300) gets its `fail` call, once. A missing endpoint, an unknown constructor,
  wrong reply types, a `{i}` beyond the event's arguments or a `$.field` in `fail` stop the host at startup.

## Rust library API

The `kern` crate (`kern/`) is a library as well as the binary. The AST is plain enums
(`Expr`, `Stmt`, `Need`, `Emit`); maps keep insertion order (`IndexMap`). These are the entry points:

| API | does |
|---|---|
| `kern::syntax::parse_module(text, expand)` | text → `Module` (`.defs` name → `Def`, plus `.states`, `.invariants`, `.channels`, `.handlers`) |
| `kern::package::load(path, overrides)` | a file, a package directory or `kern.pkg` → the checked `Module` of everything it needs; each `Def` has `.module` and `.file` in a package |
| `kern::syntax::parse_expr(src, line)` | one expression → `Expr` |
| `kern::syntax::code(e)` | `Expr` → source text |
| `kern::syntax::Def` | a definition: `.params .needs .keeps .ensures .emits .examples .body .complete`, `.hash()` |
| `kern::syntax::ParseError` | a parse error, with line number |
| `kern::wf::check_module(defs)` | well-formedness and types; `Err(WellFormedError { problems })` |
| `kern::report::analyse(defs, mutation, baseline, cache)` | everything `check` does: name → `Report` (`guarantees`, `examples`, `props`, `mutation`, `clean`, …) |
| `kern::prove::Prover` | the proofs of one definition (`Prover::new(defs).prove(d, first_kill)`): guarantees, plus the callees relied on |
| `kern::run::Interp` | the interpreter: `Interp::new(defs).run(name, args, world)` → the outcome; the world is updated or rolled back |
| `kern::run::World` | `bal`, `own`, `clock`, `audit`, `maps`, `out`; `World::from_example(w, now, maps)` |
| `kern::check::run_examples` | run a definition's `eg` lines |
| `kern::check::proptest` | random tests of a definition's guarantees |
| `kern::check::mutation_score` | spec strength |
| `kern::check::spec_mutation_score` | spec refusals (S8): `Report.spec_mutation` |
| `kern::infer::uses` | inferred effects |
| `kern::infer::fails` | inferred failures |
| `kern::infer::writes` | inferred written balances and state |
| `kern::views::iface`, `kern::views::spec`, `kern::views::editor` | the agent views (text) |
| `kern::views::tokens` | token count (None without a tokenizer) |
| `kern::render::page` | the proof page (HTML) |
| `kern::render::terminal` | the `check` output |
| `kern::baseline::write`, `kern::baseline::load` | the ratchet |
| `kern::why::load`, `kern::why::status` | the rationale store |
| `kern::host::Host` | the host: `Host::new(defs, reports, options)`, `.init(world)`, `.call(name, args)`, `.flush()`, `.pump(extra)`, `.replay()`, `.manifest()` |
| `kern::host::load_config` | parse and validate a `.host` file |
| `kern::host::SimNet` | a hostile simulated network for tests |
| `kern::emit_rust::emit`, `kern::emit_rust::build` | Kern → a Rust crate in `build/`; `cargo build` it → the binary's path |
| `kern::mcp::handle` | one MCP JSON-RPC message → the response |
| `kern::mcp::Session` | one MCP session (`Session::default()`, `.handle(msg)`): sends a repeated callee spec as a stub |

A minimal program:

```text
let defs = kern::package::load("examples/escrow/escrow_s1.k", &Default::default())?;
let reports = kern::report::analyse(&defs, false, None, None)?;
for (name, r) in &reports {
    println!("{name}: {}", r.clean);
}
```

## Common errors

| message | fix |
|---|---|
| `need must say what it fails with` | write `need COND else Ctor` |
| `` `x` shadows an existing name `` | pick a new name; names are never reused, even across branches |
| `` specs may only read state; `ledger.debit` has an effect `` | specs describe; move the effect into the body |
| `` `ledger.balance'` (after-state) is only allowed in `ensure` `` | use the after-state in `ensure` only |
| `` `when` is only allowed at the end of an `ensure` or `emit` `` | for a conditional need, write `need C2 or COND else …` |
| `` `/` needs a positive number on the right `` | arithmetic is linear: divide by a literal |
| `every path must set result` | a function needs `result = …` in both branches |
| `` invariant …: `…` must read state at `x` only `` | an invariant may only read state at its own bound key |
| `` `as F`: … `` / `as` references loop | `need as` / `emit as` must name a definition with the right number of arguments |
| `recursive (f -> g -> f)` | no recursion yet (there are no termination measures) |

## Loops

A loop states what holds at the start of each iteration, with `k` as the next index:

```kern clean
state hold Int

invariant o:Int hold.get o >= 0

clear lo:Int hi:Int
  ok otherwise
  keep ledger.total
  ensure hold.get' j = 0 for j from lo to hi
  eg hold.1:5 hold.2:7 | clear 1 3 -> ok | hold.1:0 hold.2:0
body
  for k from lo to hi
    invariant ledger.total' = ledger.total
    invariant hold.get' j = 0 for j from lo to k
    hold.set k 0
```

**What the prover checks:**
- The invariants hold when the loop starts.
- One arbitrary iteration keeps them.
- After the loop they hold with k = max(LO, HI).

**What's inferred:**
- A map written only at the loop key keeps every key outside [LO, HI), and callers can rely on
  that (`writes hold.[lo,hi)`).
- Module invariants are kept across the loop automatically, but only as long as every writer of
  the store proves them; otherwise the proofs show `!`.

**Known gaps:**
- **Events emitted inside a loop can't be described.** The page says so.
- **There's no sum,** so "the pool gains the sum of the swept holds" can't be stated (only
  conservation between two accounts can).
- **The frame can't be stated, only inferred,** so a mutant that sweeps one key too many survives
  in the definition itself (`examples/batch/batch.k`).

## Limits worth knowing

- **Types:** Int, Account, Text, and records of those as parameters. Text has only `=`, `length` and `+`.
  There's one hard-coded ledger, plus declared state. Loops are bounded, may nest, and are still a proposal (see Loops).
- **A refusal after a write can't be inherited.** Restate it as a `need` on the caller.
- **Exact audit specs grow with the call tree, and each `if` doubles the paths.** Mutation
  testing is ~95% of a full `check`. Use `--quick` and the cache in the inner loop
  (docs/research/ROADMAP_RUN.md, S2).
- The full list is in CLAUDE.md, "Known limitations".

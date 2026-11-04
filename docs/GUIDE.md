# How to Kern

A working guide: set up, write a module, read what `check` says, fix it, keep it honest, run it.
The language itself is in [REFERENCE.md](REFERENCE.md); read that first. This guide covers
the tools, the file formats and the Python API.

`tests/test_docs.py` keeps this file honest. Every `kern` code block must parse and
check. Every CLI command and option must be documented here, and every API name listed must
exist. If you change any of them, update this file in the same commit (see
[AGENTS.md](../AGENTS.md)).

## Set up

```text
python3 -m venv .venv
.venv/bin/pip install z3-solver tokenizers anthropic==0.34.2 pytest
.venv/bin/pytest                       # the whole suite; strict xfails are known gaps
```

`z3-solver` is required. `tokenizers` and `anthropic` only provide token counts.
`pip install -e .` from the repository root installs the same thing plus a `kern` command (see
`pyproject.toml`). Validation tools and experiments aren't part of it: see
[validation/README.md](../validation/README.md) and [experiments/README.md](../experiments/README.md).

## The loop

1. **Write the spec before the body.** Write the `need`s, then `ok otherwise` if callers may
   rely on *when* the call succeeds, then `keep`/`ensure`/`emit`, and one or two `eg` lines.
2. **Check:** `python -m kern check FILE.k`. Everything must read `All guarantees proven`.
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
4. **Read a `?`, `!` or ◐ as a spec too weak to prove from.**
   - `?` with "not reproduced": a callee's spec doesn't say enough, so strengthen the
     callee's `ensure`.
   - `!`: a callee isn't proven, so fix the callee first.
   - "relies on X, whose spec does not say what it emits / when it refuses": give X an
     `emit` or an `ok otherwise`.
5. **Read `missed:` lines.** Each is a body change no guarantee would notice. Add the
   guarantee that pins it, or accept it if the mutant is equivalent (it changes nothing).
6. **Ratchet:** `python -m kern baseline FILE.k` records, in `FILE.strength`, the
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

Examples: `examples/shop/`, `examples/library/`, `examples/lottery/`.

## `python -m kern` commands

| command | does |
|---|---|
| `check FILE.k` | prove, run the examples and random tests, measure spec strength; exit 1 if anything is not clean. In a package (or a package directory), every file of the package, grouped by module |
| `view FILE.k NAME --as iface\|spec\|editor` | what an agent reads (token count on stderr). `editor` = the full definition plus its callees' spec views |
| `why FILE.k NAME` | the rationale entries for NAME, with staleness |
| `anchor FILE.k` | re-anchor the `.why` file to the current builds (confirm the entries still hold first) |
| `run FILE.k "NAME ARGS" --world "A:500@u1 …"` | run one call in the interpreter |
| `baseline FILE.k` | record the ratchet baseline (`FILE.strength`) |

| option | for |
|---|---|
| `--why FILE.why` | the rationale store (default `FILE.why` next to the module; `/dev/null` for none) |
| `--html OUT.html` | `check`: also write the proof page |
| `--as iface\|spec\|editor` | `view`: which view |
| `--world WORLD` | `run`: the world, in `eg` syntax |
| `--baseline FILE.strength` | `check`: compare against this baseline (default `FILE.strength`) |
| `--quick` | `check`: no mutation testing (most of the time); strength reads "not measured" unless the cache has it for exactly this code |
| `--cache DIR` | `check`: reuse results of definitions whose code (and everything they reach) is unchanged (default `.kern-cache`, `''` = off) |
| `--against REV` | `check`: take the baseline from git at REV (`git show REV:FILE.strength`), so rewriting the file can't hide a change |

## `python -m kern.host` commands: run a module for real

| command | does |
|---|---|
| `manifest FILE.k` | the effects and network endpoints the module can reach, and which definitions are served |
| `repl FILE.k --db X.db --world "…"` | one call per line on stdin; events are delivered after commit |
| `serve FILE.k --db X.db --port 8080` | HTTP: `POST /NAME` with a JSON object of the parameters, `GET /state`, `GET /manifest` |
| `replay FILE.k --db X.db` | re-run the call log from genesis; it must match the database |
| `page FILE.k --html OUT.html` | the proof page plus a Host section (everything marked *assumed*) |

| option | for |
|---|---|
| `--db X.db` | the SQLite database (default in memory) |
| `--world WORLD` | the genesis world (only on a new database) |
| `--port N` | `serve` |
| `--no-gate` | also serve definitions that aren't proven clean (the invariants are still checked at runtime) |
| `--config FILE.host` | the network endpoints (default `FILE.host` next to the module) |
| `--skip-proven` | skip the per-call invariant check for definitions whose invariant goals are all proven ✔ (the manifest says which). Off by default: the check is on for every definition until the loop and handler fuzzer has run clean (decided 2026-11-04) |
| `--audit-rate R` | with `--skip-proven`: still run the skipped checks on a fraction R of calls (defence in depth; the counts are in the manifest). The full invariant audit at startup always runs |
| `--html OUT.html` | `page` |

- **One call is one transaction.** A refusal writes nothing, and an unproven definition isn't
  served. HTTP status codes: ok → 200, refused → 409, bad call → 400, unproven → 403, broken
  invariant → 500.
- **Delivery is at-least-once,** so consumers dedupe on `seq`.
- **Assumed handlers** (`handler rand.below …`):
  - the host must provide each one, or it refuses to start;
  - every value is checked against the assumed spec, and a violation is an atomic error;
  - every value is recorded in `log.observed`, so `replay` feeds the same values back.
- **Network replies arrive as new calls.** Details are in docs/HOST.md.

## The MCP server: working through views

`python -m kern.mcp` serves the tools below over stdio (MCP, JSON-RPC 2.0, no dependencies), so
an agent works through views instead of files. To register it with Claude Code:
`claude mcp add kern -- .venv/bin/python -m kern.mcp`, or pass the same command with
`--mcp-config`.

| tool | does |
|---|---|
| `kern_list` | the definitions: interface line and status |
| `kern_view` | `iface`, `spec` or `editor` view of one definition (identical to `kern view`) |
| `kern_callers` | who reaches a definition, so you know whose guarantees an edit can break |
| `kern_why` | the rationale entries, with staleness |
| `kern_check` | a summary (status per definition plus what's broken); `name` gives full detail; `quick` (default) skips mutation testing |
| `kern_run` | one call in the interpreter |
| `kern_edit` | several `add` / `replace` / `remove` operations, checked once; if the result is ill-formed, nothing is written |
| `kern_add`, `kern_replace`, `kern_remove` | one structural edit. `expect_hash` refuses if the definition changed underneath |

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

## `python -m kern.emit_rust`: compile to Rust

`python -m kern.emit_rust FILE.k` writes a Rust crate for the module to `build/<module>/`, linked
with the runtime crate `kern-rt` in `runtime/`. `need`s compile to code. `keep`, `ensure` and
`emit` are proven, so they compile to nothing. `kern/run.py` stays the reference semantics: the
runtime is wrong wherever the two disagree. The contract and the known divergences (for example,
Int is i64 at run time) are in [research/RUNTIME_PLAN.md](research/RUNTIME_PLAN.md). Needs `cargo`.
It reads single files only, not packages yet.

| option | for |
|---|---|
| `--out DIR` | where to write the crate (default `build/<module>`) |
| `--build` | also `cargo build --release`; prints the binary's path (cargo's target is `build/target`) |

## Files

- **`FILE.k`:** the module.
- **`kern.pkg`:** marks a package root (it may be empty). In a package, the `.why` and `.strength`
  files are per package: `<root>/kern.why` and `<root>/kern.strength`.
- **`FILE.why`:** rationale, never obligations. Each block is a definition anchored to its
  build hash, followed by indented entries:

  ```text
  transfer@4be917d8
    Insufficient carries the shortfall so support can tell the customer how much is missing
  ```

  An entry goes stale when the definition or its inferred interface changes. An entry that
  reads like a rule ("must …") is flagged: move it into the spec.
- **`FILE.strength`:** the ratchet baseline, written by `kern baseline`. Don't edit it by hand.
- **`FILE.host`** (in a package, `<root>/kern.host`): one line per network request constructor:

  ```text
  Fetch -> GET http://127.0.0.1:9/limit/{1}; reply got_limit {1} {2} $.value; fail limit_failed {1} {2}
  ```

  `{i}` is the event's i-th argument and `$.field` an Int from the JSON reply. A missing
  endpoint, an unknown constructor or wrong reply types stop the host at startup.

## Python API

Everything is plain Python and the AST is tuples. These are the entry points:

| API | does |
|---|---|
| `kern.syntax.parse_module(text)` | text → `Module` (a dict of name → `Def`, plus `.states`, `.invariants`, `.channels`) |
| `kern.package.load(path)` | a file, a package directory or `kern.pkg` → the checked `Module` of everything it needs; each `Def` has `.module` and `.file` in a package |
| `kern.syntax.parse_expr(src)` | one expression → AST tuple |
| `kern.syntax.code(e)` | AST → source text |
| `kern.syntax.Def` | a definition: `.params .needs .keeps .ensures .emits .examples .body .complete .hash` |
| `kern.syntax.ParseError` | a parse error, with line number |
| `kern.wf.check_module(defs)` | well-formedness and types; raises `WellFormedError` (`.problems`) |
| `kern.report.analyse(defs, mutation=True, baseline=None, cache=None)` | everything `check` does: name → report (`guarantees`, `examples`, `props`, `mutation`, `clean`, …) |
| `kern.prove.Prover(defs).prove(d)` | the proofs of one definition: `[{kind, clause, status, counterexample?}]`, plus the callees relied on |
| `kern.run.Interp(defs).run(name, args, world)` | one atomic call → `(outcome, world)` |
| `kern.run.World` | `bal`, `own`, `clock`, `audit`, `maps`, `out`; `World.from_example(w, now, maps)` |
| `kern.check.run_examples(defs, d)` | run d's `eg` lines |
| `kern.check.proptest(defs, d)` | random tests of d's guarantees |
| `kern.check.mutation_score(defs, d)` | spec strength |
| `kern.infer.uses(defs, d)` | inferred effects |
| `kern.infer.fails(defs, d)` | inferred failures |
| `kern.infer.writes(defs, d)` | inferred written balances and state |
| `kern.views.iface(r, why)`, `kern.views.spec(r, why)`, `kern.views.editor(reports, name, whys)` | the agent views (text) |
| `kern.views.tokens(text)` | token count (None without a tokenizer) |
| `kern.render.page(reports, whys, title)` | the proof page (HTML) |
| `kern.render.terminal(reports, whys)` | the `check` output |
| `kern.baseline.write(path, reports)`, `kern.baseline.load(path)` | the ratchet |
| `kern.why.load(path)`, `kern.why.status(store, d, defs)` | the rationale store |
| `kern.host.Host(defs, db, …)` | the host: `.init(world)`, `.call(name, args)`, `.flush()`, `.pump()`, `.replay()`, `.manifest()` |
| `kern.host.load_config(text, defs)` | parse and validate a `.host` file |
| `kern.host.SimNet(endpoints, answer, seed)` | a hostile simulated network for tests |
| `kern.emit_rust.emit(path)`, `kern.emit_rust.build(crate)` | Kern → a Rust crate in `build/`; `cargo build` it → the binary's path |

A minimal script:

```text
from kern.syntax import parse_module
from kern.wf import check_module
from kern.report import analyse
defs = check_module(parse_module(open("examples/escrow_s1.k").read()))
reports = analyse(defs, mutation=False)
print({n: r["clean"] for n, r in reports.items()})
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
  in the definition itself (`examples/batch.k`).

## Limits worth knowing

- **Types:** Int and Account only. No text or records. There's one hard-coded ledger, plus
  declared state. Loops are bounded, can't be nested, and are still a proposal (see Loops).
- **A refusal after a write can't be inherited.** Restate it as a `need` on the caller.
- **Exact audit specs grow with the call tree, and each `if` doubles the paths.** Mutation
  testing is ~95% of a full `check`. Use `--quick` and the cache in the inner loop
  (docs/research/ROADMAP_RUN.md, S2).
- The full list is in CLAUDE.md, "Known limitations".

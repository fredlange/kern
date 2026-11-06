# Host: how Kern reaches the operating system

Kern code never touches the OS. A **host** does, and keeps the three properties the proofs
rest on: atomicity, a closed world of effects, and determinism. Prototype: `kern host`
(`kern/src/host.rs`: SQLite via rusqlite, bundled; an HTTP server and client). Tests: `kern/tests/host.rs`.
Until 2026-11-05 the host was Python (`kern/host.py`); the measurements below that predate that date
were taken on it, and the Rust host keeps its database format (docs/research/RUST_PORT_PLAN.md).

## Effects, sorted by what they do to the proofs

| Kind | Examples | Atomicity | Prover | In the prototype host |
|---|---|---|---|---|
| Owned state | ledger, `state` maps | rolled back | modelled exactly | SQLite tables `account`, `cell`; one transaction per call |
| Observation | wall clock, stdin, HTTP reply, file read | nothing to undo | fresh symbolic value | `clock.now` = wall clock, read once per call, recorded in `log` |
| Action | print, send, write file | buffer, release after commit | `emit` sequence | `audit.emit` → table `outbox` → sinks (stdout) after commit |
| Request/response inside one call | "fetch, then decide" | impossible | impossible | not allowed; split into two calls (decide + emit request; a later call takes the reply as an event) |

## What the host does

```
kern host manifest examples/escrow/escrow_s2.k
kern host repl     examples/escrow/escrow_s2.k --db e.db --world "A:1000@u1 E:0@u0 B:0@u2 H:0@u9"
kern host serve    examples/escrow/escrow_s2.k --db e.db --port 8080
kern host replay   examples/escrow/escrow_s2.k --db e.db
```

- **Proof-gated:** at startup the host runs the checks (`analyse`, no mutation: 0.2 s for
  escrow_s2's 26 defs, 2026-11-05; the Python host took 0.83 s) and refuses to run any definition that is not clean (HTTP 403).
  `--no-gate` turns that off.
- **One call = one transaction:** `begin immediate` → load world → `Interp::run` →
  runtime invariant check → write the diff, log entry, outbox rows → `commit`. A refusal or
  runtime error writes only the log entry. Calls are serialised with a lock (the proofs
  assume one top-level call at a time).
- **HTTP:** `POST /NAME` with a JSON object of exactly the def's parameters; a record parameter `p:Point` is
  one object with exactly its fields, `{"p": {"x": 1, "y": 2}}`, and a nested one (`l:Line`, `record Line
  a:Point b:Point`) an object inside it, `{"l": {"a": {"x": 1, "y": 2}, "b": {...}}}` (the host decodes it into the
  flattened `p.x p.y`, `l.a.x l.a.y l.b.x l.b.y`; `repl` and `.host` reply ARGS stay positional). `ok` → 200
  (with the emitted events), declared refusal → 409 `{"refused": Ctor, "args": [...]}`,
  bad call (wrong type, unknown account, a number that isn't an i64 such as `1.5`) → 400, not proven → 403,
  runtime invariant broken (only possible with `--no-gate`) or an Int out of range (below) → 500.
- **Text (2026-11-05).** A Text state map (`state title Int -> Text`) is stored in SQLite table `textcell`
  (state, key, value text), read lazily like `cell`, written with the call; a missing entry reads `""`. The genesis
  world takes `title.1:"…"` entries (the `eg` world grammar), `repl` takes quoted Text arguments
  (`post 1 "hello world"`), HTTP takes JSON strings for Text parameters, and `GET /state` shows Text maps with
  string values. `replay` compares Text state too. A `.host` reply `$.field` for a Text parameter of the reply
  definition takes a JSON string (an Int parameter still takes an integer; anything else is the `fail` call), and
  a Text in a request URL is %-encoded. `repl` prints a reply's Text arguments quoted the same way, with line
  breaks escaped (`\n`, `\r`), so each reply is one line: `reply answer 3 "why?" "because" -> ok`.
- **Records in state (2026-11-06).** `state order Int -> Order` is one state per field (`order.item`, `order.qty`;
  lang's flattening), so SQLite keeps a row per field in `cell` (Int fields) or `textcell` (Text fields), named
  `order.qty`, as the interpreter's world does, and replay compares them field by field. The genesis world names
  field states (`order.qty.7:2`, `seg.b.x.1:3`). `GET /state` and `repl`'s `state` show the record, not its fields:
  `"order": {"7": {"item": 3, "qty": 2, "note": "rush"}}`, nested records nested, each key any field has shown with
  every field (absent ones at 0 or `""`); the shape a record argument has (DECISIONS 2026-11-06). The manifest
  and the page say which tables a state uses (`cell`, `textcell` or both); before, a Text state was shown as `cell`.
- **Record results (2026-11-06).** A definition returning a record (`mid p:Point q:Point -> Point`) answers HTTP
  with the record as an object, nested as a record argument is: `"result": {"x": 2, "y": 3}`. `repl` prints its
  leaves as an `eg` line asserts them, `ok 2 3` (a Text leaf quoted, an Account by name). Replay is unchanged (the
  value is recomputed). `kern build` returns it as a tuple of its leaves and binds `r = f a` / `q = Point a b` as one
  local per leaf; the binary's script protocol prints `ok result=2 3`.
- **Recursion (2026-11-05).** A recursive definition runs once its `decreases` measure is proven; calls nest at
  most 200 deep in one call, the top-level one included (the interpreter's `MAX_CALL_DEPTH`; kern-rt's is the same
  number). Deeper is a resource error, like the iteration limit: undone, logged, skipped by replay.
- **Definitions that write and return (2026-11-06).** A `-> Int` definition that writes, emits or refuses runs
  like any other call; its value is the HTTP reply's `"result"` (`{"status": "ok", "events": [...], "result": 1}`)
  and `repl` prints it as an `eg` line asserts it, `ok 1` (any definition with a result does). Nothing new is
  logged: the value is a function of the state before the call, the arguments and the recorded handler values, so
  replay gives it back. `kern build` compiles them as every definition (all go through the transaction, so a
  refusal after a bound call `x = f a` undoes `f`'s writes too); difftest compares the value (`ok result=N`).
- **`clock.tick` (S10, 2026-11-05).** Inside a call it adds exactly 1 to `clock.now` (the interpreter's
  semantics; replay re-runs it, nothing new is recorded). The host reads the wall clock once per call, so after a
  call that ticked, the next call's `now` is at least where that call ended plus 1 (meta `clock_floor`, kept across
  restarts): time never goes back across calls. A module that never ticks sees the wall clock exactly as before.
  `kern build` programs tick the same way (kern-rt `Tx::tick`).
- **One Int range, i64 (S20, 2026-11-05).** SQLite stores i64 and the compiled runtime computes in i64; the
  interpreter computes in i128. A successful call whose stored balance, map key or value, event or result, or a
  refusal whose payload, leaves i64 is a host fault (`"fault": "range"`): rolled back, logged as an error, which
  replay skips. Before, it was stored wrapped and `replay` reported "state differs". Input past i64 (HTTP, `repl`,
  the genesis world) is refused, never wrapped or read as 0. Integer literals in Kern source are lang's side.
- **An iteration limit per call (S21, 2026-11-05).** At most 10,000,000 loop and sum iterations per top-level call
  (`ITERATION_LIMIT`), charged as the interpreter's budget is: `hi - lo` when a `for` or `sum` starts, so a huge
  bound fails at once instead of holding the write lock. Past it the call is a host fault (`"fault": "limit"`,
  HTTP 500): undone, logged as an error, which replay skips. A `kern build` program has the same limit
  (kern-rt `ITERATION_LIMIT`, `Tx::spend`) and faults at the same call. The manifest states it. The proof says a
  call succeeds, not when; this bounds the when. The genesis world must satisfy the invariants. `GET /state`, `GET /manifest`.
- **Console:** `repl` takes one call per line on stdin, prints the outcome and delivers the outbox
  to stdout. stdin is the event source and stdout is a sink.
- **Permission manifest:** the inferred `uses` of every def, plus how the host realises each
  effect. The module cannot name anything else. *The enforcement is the closed handler table,
  not an OS sandbox:* the host process itself can do anything. A real sandbox is
  roadmap 4 (Wasm/WASI).
- **Replay:** the log records every attempted call with its `now`. `replay` re-runs it from the
  genesis world and compares the state and the outbox with the database.

## Measurements (2026-11-03)

| Claim | Result |
|---|---|
| SQLite rollback = interpreter `_restore` | 600 random escrow_s2 calls (≥100 ok, ≥100 refused), host vs in-memory `Interp` with the same clock: every outcome, final state and outbox are identical (`test_sqlite_host_matches_the_interpreter`) |
| Proofs survive a real clock | *By reading `prove.py`, not by a new measurement:* the prover treats `now` as one symbolic value per call, so no proof can depend on ticking. With a wall clock (seconds), many calls share one `now`. Replay of real-clock runs matched |
| Replay is deterministic | Matches after the random run and after real HTTP runs; a hand-edited balance is detected |
| Delivery | **At-least-once, not exactly-once.** A crash between delivering and marking gives a duplicate (`test_delivery_is_at_least_once`: seq 2 delivered twice). Consumers must dedupe on `seq` |
| Throughput (`transfer`, 50 accounts) | Before H1: 0.10 ms/call in memory, 0.34 ms/call file-backed SQLite. After H1: 0.034 / 0.28 ms (scorecard item 2) |
| Scaling | Before H1: **linear in world size**, 0.10 / 0.85 / 7.98 ms per call at 50 / 500 / 5000 accounts, because every call loaded the whole world and checked invariants over every key. After H1: flat, see the scorecard below |

## Known gaps, in the order they bite

1. ~~**Whole-world load per call.**~~ Fixed in H1: a call reads SQLite lazily, and the
   runtime invariant check covers only the keys the call wrote.
2. ~~**No outbound effects beyond audit.**~~ H2 added named channels: `channel console` (module
   level), `console.emit (Line x)` in a body and `emit console Line x` in a spec. Each channel is
   an exact sequence, the order across channels is unspecified, and a channel the spec never
   names is unconstrained. The outbox has a `channel` column; `console` goes to stdout.
3. **The prover crashes on unknown handlers** (still true; H2/H3 needed none) (`prove.py`, `raise ValueError("prover: unknown …")`),
   Declared handlers with assumed specs (roadmap 1) are needed only for observations *inside* a call
   (environment variables, file reads, randomness). Console and network don't need them if output is a
   channel `emit` and replies arrive as new calls; see [HOST_PLAN.md](research/HOST_PLAN.md).
4. ~~**Effect labels have no resource.**~~ The manifest now lists every network endpoint, from the
   host config. File access still has no effect at all.
5. ~~**Int and Account only.**~~ Text since 2026-11-05 (below); records arrive as nested JSON objects. The host
   decodes at the boundary (JSON strings for Text, `$.field` by the reply definition's parameter type), so Kern
   never parses bytes. Still missing: other types (lists, …).
6. **No accounts created at runtime.** Accounts only come from the genesis world.

## Scorecard for HOST_PLAN.md (filled in step by step)

| # | Claim | Result |
|---|---|---|
| 1 | Host ≡ interpreter | **pass** after H1: `test_sqlite_host_matches_the_interpreter` (600 random escrow_s2 calls) |
| 2 | Lazy state fixes the scaling | **pass**. `transfer`, 300 calls, 300/300 ok, replay matches. In memory: 0.034 / 0.033 / 0.031 ms per call at 50 / 500 / 5000 accounts (0.9×; before H1 it was 75×). File-backed SQLite: 0.282 / 0.327 / 0.330 ms (1.17×). A timing-free check: the SQL statements per call are the same at 50 and 5000 accounts (`test_a_call_does_not_read_the_whole_world`) |
| 3 | Channels cost nothing for existing code | **pass** after H2: escrow_s1/s2/s3 are clean (10/10, 26/26, 55/55); spec-view token counts and build hashes of every definition are identical to before H2 (1626 / 4572 / 9198 tokens) |
| 4 | A channel is cheap in a spec | **pass**: `emit console Line x` costs 6 tokens vs 5 for `emit Line x` (+1). HOST.md's earlier `emit console.Line x` would cost +2 |
| 5 | A hostile network can't break the invariants | **pass.** 1000 seeded runs of `examples/limits/limits.k`: 25,000 calls, 5,394 replies (dropped, duplicated, delayed, released out of order, failed, or garbled: negative, non-Int, missing). **0** invariant faults, **668/668** duplicate or stale replies refused with `Stale`, and `replay` matches **1000/1000** runs. Plus one run against a real local `HTTPServer` (a 200 sets the limit, a 500 becomes `limit_failed`) |
| 6 | The manifest is the policy | **pass.** `manifest` lists every endpoint (`net: Fetch -> GET http://127.0.0.1:9/limit/*  reply got_limit, fail limit_failed`). The host refuses to start, exit 2, when a request has no endpoint, when there is no config, when an endpoint names a constructor the module never sends, and when a reply or fail definition is missing or has the wrong types |
| 7 | Network needs no declared handlers | **pass.** `prove.py`: +51 / −27 lines in H2, all channel-keyed `emit` (per-channel event lists, per-channel goals, per-channel opaque callees), and **0 lines** in H3. No handler was declared or modelled for the network |
| 8 | Crash recovery | **pass.** A real child process flushes 40 committed events through a sink that writes, fsyncs and then sleeps. That puts every kill inside the window between delivering and marking, which is the worst case. Three `kill -9`s mid-flush, then a restart: all 40 delivered, 43 deliveries, **3 duplicates = 1 per kill** |

**How H1 works:**
- **Lazy world.** `host.LazyWorld` is a `World` whose stores read SQLite on demand. A read cache is
  shared within one call; it's valid because the call holds `begin immediate`, and it's never kept
  across calls. A written overlay records what the call changed.
- **`run.py` didn't need a "skip the snapshot" flag.** `copy()` copies only the overlay, so
  `Interp.run`'s snapshot and in-place rollback work unchanged.
- **`ledger.total`** is SQL `sum` plus the overlay's delta.
- **Commit** writes the overlay.

**Key-scoped invariant check:**
- `Interp.broken_invariants(world, keys)` checks only the keys the call wrote. At runtime those keys
  are exact, including computed ones, so this is sound for every definition, not only those the
  prover could check key by key.
- That made the plan's "keep the full check where the prover said unknown" unnecessary. The test
  is `test_key_scoped_invariant_check_catches_a_write_at_any_key`.
- **Soundness condition, now enforced:** `wf.invariant_problems` rejects any invariant that reads
  state anywhere but its own bound key. Rejected: `ledger.total`, `clock.now`, `hold.get 3`,
  `hold.get (o + 1)`. Every escrow invariant passes.
- **The compiled program does the same check, where it isn't proven (2026-11-05).** `kern build` runs the
  prover (no mutation testing) and, for each definition, compiles a check of every module invariant that has a
  goal for it which isn't ✔ (○, ✗, ?, !), at the keys the call wrote (the runtime's undo log), after the body.
  A broken one undoes the call as a fault. An invariant with no goal reads nothing the definition writes (the
  inferred frame), and a ✔ one is proven, so neither is checked: a fully proven module compiles to the same
  code as before. Unlike the host's default, which checks every definition unless `--skip-proven`. Like
  `--skip-proven`, it relies on the starting world satisfying the invariants (the host audits it at startup; a
  compiled program's `world` line isn't audited). Tests: `emit_rust::tests`, kern-rt's
  `written_keys_come_from_the_undo_log_once_each`.

**How H2 works:**
- **Interpreter and prover:** `World.audit` and `Sym.audit` are unchanged; other channels live in
  `.out[channel]`.
- **Spec clauses** get a parallel `Def.emit_channel` map, folded into the hash only when non-empty
  (that's why item 3 holds by construction). `emit as` carries the callee's channels.
- **Effect label per channel** (`console`), so `uses` and the manifest separate them.
- **Host:** delivers audit first, then channels in declaration order. That is a convention, and
  the page says the order across channels isn't guaranteed.

**How H3–H5 work:**
- **Requests are events on `net`.** The `.host` file next to the module (`limits.host`) maps each
  request constructor to `METHOD URL; reply DEF ARGS; fail DEF ARGS[; timeout N]`. ARGS are `{i}` (the event's
  i-th argument) or `$.field` (an Int field of the JSON reply; `reply` only, since a failure has no body).
- **Replies are calls.** After commit, `flush` sends each request. `pump` runs whatever comes back
  as an ordinary logged top-level call, so replay covers it. Kern never sees bytes: a timeout, a
  non-2xx status or a non-Int becomes the `fail` call.
  `serve` and `repl` pump after every call (before 2026-11-04 only the Python API did, so under `serve` a
  reply never ran and its request stayed pending).
- **Deadlines (S12, 2026-11-05).** A request is *answered* when a call to its endpoint's `reply` or `fail`
  definition returns `ok` with the request's `{i}` arguments (in that call's transaction; the outbox row's
  `answered` column holds the call's `seq`). A request still unanswered `timeout N` seconds after the call that
  sent it (default 300, `DEFAULT_TIMEOUT`) gets its `fail` call, once, as an ordinary logged call, so `replay`
  covers it; `repl` prints it as `timeout limit_failed A 1 -> ok`. Seconds of the *recorded* clock: `pump` compares
  the latest `now` in `log` with the sending call's `now`, after the replies that did arrive have run, so a reply in
  the same flush is never preempted and no extra clock read is made. An idle `serve` therefore fires a deadline
  at its next call, not on the wall clock. A refused reply (BadValue, a garbled body) doesn't answer the request,
  so it times out too; a reply that arrives after the timeout is an ordinary call, which limits.k refuses as
  `Stale`. The mark is written after the timeout call, so a crash in between fires it again (at-least-once,
  like delivery). Databases from before the column count their old requests as answered. Tests: the gap test
  `s12_a_request_with_no_reply_times_out_into_a_call` (kern/tests/shortcomings.rs) and `host::tests` in
  kern/src/host.rs (silence, an ok reply, a refused then late reply, a restart, an old database, config).
- **Simulation:** `SimNet` is seeded. It drops, duplicates, delays or fails each reply, and
  `release()` lets held-back replies go in a random order.
- **The page:** `kern host page FILE.k --html X` writes the proof page plus a Host
  section: effects, endpoints and assumptions, every line marked *assumed*, none ✔
  (`render::host_html`, fixed templates). Since S12 the assumptions end with each endpoint's effective
  deadline (`Request deadlines: Fetch 300 s (the default).`), and the manifest's `net:` line says the same.
  It reads the module's `.why` file as `kern check` does (`<module>.why`, in a package `<root>/kern.why`);
  before 2026-11-05 it rendered with an empty one.

**Findings:**
- **The plan's demo design needed one change to pass item 5.** With only `pending` as a flag, a
  delayed reply to an *old* request would be accepted while a *newer* request is pending. In
  `limits.k` each request carries a sequence number (`req.get a`), and `got_limit` and
  `limit_failed` need both `pending = 1` and a matching number.
- **Liveness wasn't covered until S12.** A dropped or refused reply (BadValue) left `pending = 1`, and
  `request_limit` then refused `AlreadyPending` for good. In the 1000 runs, **1,952 accounts ended
  pending**. Invariants held; progress didn't. **With deadlines (2026-11-05)**, the same hostile network
  (1000 seeded runs, `timeout 20`, one clock second per call; `host::tests::measure_liveness_on_a_hostile_network`,
  run with `--ignored`): 31,529 calls, 2,092 of them timeout calls, 0 invariant faults, 0 replay mismatches;
  1,262 accounts pending right after the run (deadlines not yet reached), **0 once every deadline has
  passed**. Not the same random stream as the 1,952 (a different harness and clock), so the two counts aren't
  a before/after pair; the 0 is the claim.
- **Requests are at-least-once too.** A crash during `flush` re-sends a request, so the risk
  service must tolerate duplicates. The reply side is safe (Stale); the request side relies on
  the server.
- **This is a proof of concept on one process.** There's no sandbox (the host process can do
  anything; see "Permission manifest" above), no concurrent calls, and no Text or records.

**Known gaps found while doing this, not fixed (not plan items):**
- **A channel a definition uses but never specifies is unconstrained, and the page doesn't say
  so.** It should, the way it warns about writes no guarantee describes (F5).
- ~~**Config validation has two rough edges.**~~ Fixed with S12: a `{i}` beyond the event's arity and a
  `$.field` in `fail` ARGS are both ConfigErrors (the deadline's `fail` call is built from the event alone).
- **`net_requests` reads the constructor from the `net.emit` argument syntactically.** An event
  bound by a `let` first would be misread.

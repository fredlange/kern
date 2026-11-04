# Host: how Kern reaches the operating system

Kern code never touches the OS. A **host** does, and keeps the three properties the proofs
rest on: atomicity, a closed world of effects, and determinism. Prototype: `kern/host.py`
(stdlib only: `sqlite3`, `http.server`). Tests: `tests/test_host.py`.

## Effects, sorted by what they do to the proofs

| Kind | Examples | Atomicity | Prover | In the prototype host |
|---|---|---|---|---|
| Owned state | ledger, `state` maps | rolled back | modelled exactly | SQLite tables `account`, `cell`; one transaction per call |
| Observation | wall clock, stdin, HTTP reply, file read | nothing to undo | fresh symbolic value | `clock.now` = wall clock, read once per call, recorded in `log` |
| Action | print, send, write file | buffer, release after commit | `emit` sequence | `audit.emit` → table `outbox` → sinks (stdout) after commit |
| Request/response inside one call | "fetch, then decide" | impossible | impossible | not allowed; split into two calls (decide + emit request; a later call takes the reply as an event) |

## What the host does

```
python -m kern.host manifest examples/escrow/escrow_s2.k
python -m kern.host repl     examples/escrow/escrow_s2.k --db e.db --world "A:1000@u1 E:0@u0 B:0@u2 H:0@u9"
python -m kern.host serve    examples/escrow/escrow_s2.k --db e.db --port 8080
python -m kern.host replay   examples/escrow/escrow_s2.k --db e.db
```

- **Proof-gated:** at startup the host runs the checks (`analyse`, no mutation: 0.83 s for
  escrow_s2's 26 defs) and refuses to run any definition that is not clean (HTTP 403).
  `--no-gate` turns that off.
- **One call = one transaction:** `begin immediate` → load world → `Interp.run` →
  runtime invariant check → write the diff, log entry, outbox rows → `commit`. A refusal or
  runtime error writes only the log entry. Calls are serialised with a lock (the proofs
  assume one top-level call at a time).
- **HTTP:** `POST /NAME` with a JSON object of exactly the def's parameters. `ok` → 200
  (with the emitted events), declared refusal → 409 `{"refused": Ctor, "args": [...]}`,
  bad call (wrong type, unknown account) → 400, not proven → 403, runtime invariant broken
  (only possible with `--no-gate`) → 500. The genesis world must satisfy the invariants. `GET /state`, `GET /manifest`.
- **Console:** `repl` takes one call per line on stdin, prints the outcome and delivers the outbox
  to stdout. stdin is the event source and stdout is a sink.
- **Permission manifest:** the inferred `uses` of every def, plus how the host realises each
  effect. The module cannot name anything else. *The enforcement is the closed handler table,
  not an OS sandbox:* the Python host process itself can do anything. A real sandbox is
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
5. **Int and Account only.** Real requests need Text and records. The plan is for the host to
   decode them against a declared schema at the boundary, the same way `run()` already
   type-checks arguments, so Kern never parses bytes.
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
  request constructor to `METHOD URL; reply DEF ARGS; fail DEF ARGS`. ARGS are `{i}` (the event's
  i-th argument) or `$.field` (an Int field of the JSON reply).
- **Replies are calls.** After commit, `flush` sends each request. `pump` runs whatever comes back
  as an ordinary logged top-level call, so replay covers it. Kern never sees bytes: a timeout, a
  non-2xx status or a non-Int becomes the `fail` call.
  `serve` and `repl` pump after every call (before 2026-11-04 only the Python API did, so under `serve` a
  reply never ran and its request stayed pending).
- **Simulation:** `SimNet` is seeded. It drops, duplicates, delays or fails each reply, and
  `release()` lets held-back replies go in a random order.
- **The page:** `python -m kern.host page FILE.k --html X` writes the proof page plus a Host
  section: effects, endpoints and assumptions, every line marked *assumed*, none ✔
  (`render.host_html`, fixed templates).

**Findings:**
- **The plan's demo design needed one change to pass item 5.** With only `pending` as a flag, a
  delayed reply to an *old* request would be accepted while a *newer* request is pending. In
  `limits.k` each request carries a sequence number (`req.get a`), and `got_limit` and
  `limit_failed` need both `pending = 1` and a matching number.
- **Liveness isn't covered.** A dropped or refused reply (BadValue) leaves `pending = 1`, and
  `request_limit` then refuses `AlreadyPending` for good. In the 1000 runs, **1,952 accounts ended
  pending**. Invariants hold; progress doesn't. It needs a timeout call (for example, the host
  calls `limit_failed` after N seconds) or a `cancel_request` definition. Not built.
- **Requests are at-least-once too.** A crash during `flush` re-sends a request, so the risk
  service must tolerate duplicates. The reply side is safe (Stale); the request side relies on
  the server.
- **This is a proof of concept on one process.** There's no sandbox (the Python host can do
  anything; see "Permission manifest" above), no concurrent calls, and no Text or records.

**Known gaps found while doing this, not fixed (not plan items):**
- **A channel a definition uses but never specifies is unconstrained, and the page doesn't say
  so.** It should, the way it warns about writes no guarantee describes (F5).
- **Config validation has two rough edges.** A `{i}` beyond the event's arity raises an
  IndexError at startup rather than a ConfigError (it's still refused before anything runs). A
  `$.field` in `fail` ARGS is accepted although a failure has no body.
- **`net_requests` reads the constructor from the `net.emit` argument syntactically.** An event
  bound by a `let` first would be misread.

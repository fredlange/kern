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
python -m kern.host manifest examples/escrow_s2.k
python -m kern.host repl     examples/escrow_s2.k --db e.db --world "A:1000@u1 E:0@u0 B:0@u2 H:0@u9"
python -m kern.host serve    examples/escrow_s2.k --db e.db --port 8080
python -m kern.host replay   examples/escrow_s2.k --db e.db
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
| Throughput (`transfer`, 50 accounts) | 0.10 ms/call in-memory, 0.34 ms/call file-backed SQLite |
| Scaling | Before H1: **linear in world size**, 0.10 / 0.85 / 7.98 ms per call at 50 / 500 / 5000 accounts, because every call loaded the whole world and checked invariants over every key. After H1: flat, see the scorecard below |

## Known gaps, in the order they bite

1. ~~**Whole-world load per call.**~~ Fixed in H1: a call reads SQLite lazily, and the
   runtime invariant check covers only the keys the call wrote.
2. **No outbound effects beyond audit.** Console and network sends need `audit` generalised to
   named channels (`emit console.Line …`, `emit net.Fetch …`) with the same exact-sequence
   spec. Inbound replies come back as calls (`on Fetched …`), so failure is data.
3. **The prover crashes on unknown handlers** (`prove.py`, `raise ValueError("prover: unknown …")`),
   Declared handlers with assumed specs (roadmap 1) are needed only for observations *inside* a call
   (environment variables, file reads, randomness). Console and network don't need them if output is a
   channel `emit` and replies arrive as new calls; see [HOST_PLAN.md](HOST_PLAN.md).
4. **Effect labels have no resource** (`net.send` rather than `net.send host`), so the manifest
   can't yet narrow network or file access.
5. **Int and Account only.** Real requests need Text and records. The plan is for the host to
   decode them against a declared schema at the boundary, the same way `run()` already
   type-checks arguments, so Kern never parses bytes.
6. **No accounts created at runtime.** Accounts only come from the genesis world.

## Scorecard for HOST_PLAN.md (filled in step by step)

| # | Claim | Result |
|---|---|---|
| 1 | Host ≡ interpreter | **pass** after H1: `test_sqlite_host_matches_the_interpreter` (600 random escrow_s2 calls) |
| 2 | Lazy state fixes the scaling | **pass**. `transfer`, 300 calls, 300/300 ok, replay matches. In memory: 0.034 / 0.033 / 0.031 ms per call at 50 / 500 / 5000 accounts (0.9×; before H1 it was 75×). File-backed SQLite: 0.282 / 0.327 / 0.330 ms (1.17×). A timing-free check: the SQL statements per call are the same at 50 and 5000 accounts (`test_a_call_does_not_read_the_whole_world`) |

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

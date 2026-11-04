# Host: handover plan for the next proof of concept

The goal is a Kern module that really talks to the console and the network, without
giving up the three properties the proofs depend on: atomicity, a closed world of
effects, and determinism. The scorecard below is pre-registered, so it has to be
filled in before the result is called a success.

Background reading: [HOST.md](../HOST.md) (design, measurements, gaps), `kern/host.py`,
`tests/test_host.py`. All of it landed in commit `fac9685`.

## Where it stands (2026-11-03)

- `python -m kern.host serve|repl|replay|manifest FILE.k --db X.db`.
- SQLite state with one transaction per call, and an `audit` outbox delivered after commit
  (at-least-once).
- Wall clock, recorded per call. HTTP: ok → 200, refused → 409, bad call → 400,
  unproven → 403, broken invariant → 500.
- Only clean definitions are served. Replay works.
- On escrow_s3, all 55 defs are served and startup takes about 2 s.
- The host behaves exactly like the interpreter on 600 random escrow_s2 calls.
- Per-call cost is linear in world size: 0.10 / 0.85 / 7.98 ms at 50 / 500 / 5000 accounts.
- The only output is `audit` to stdout. Nothing goes to the network.

## A finding that changes the order in HOST.md

HOST.md lists "handlers declared in Kern" as a prerequisite for new effects. **That is not
needed for console or network** if the design rule below holds:

> Output goes through `emit` on a named channel and is delivered after commit. Input,
> including network replies, arrives as a **new top-level call**.

Under that rule a call never waits on the world, so the prover sees no new handlers.
Only `emit` becomes keyed by channel. Declared handlers with assumed specs are needed
only for *observations inside a call* (environment variables, file reads, randomness),
and those are left out of this proof of concept. Scorecard item 7 tests this claim.

## Pre-registered scorecard

Fill this in in HOST.md when the work is done. Report items that fail too, with the numbers.

| # | Claim | Pass condition |
|---|---|---|
| 1 | The host is still equivalent to the interpreter | `test_sqlite_host_matches_the_interpreter` passes after every step |
| 2 | Lazy state fixes the scaling | ms per call at 5000 accounts ≤ 2× the figure at 50 (today it is 80×) |
| 3 | Channels cost nothing for existing code | escrow_s1/s2/s3: every def stays clean, and the spec-view token counts are unchanged |
| 4 | A channel is cheap in a spec | `emit console Line x` costs ≤ 1 token more than `emit Line x` (measure with `views.tokens`) |
| 5 | A hostile network can't break the invariants | Over 1000 simulated runs where replies are dropped, duplicated, reordered and delayed: 0 invariant breaks, every duplicate reply refused (`Stale`), and `replay` matches every run |
| 6 | The manifest is the policy | The manifest lists every endpoint the module can reach, and a request constructor with no endpoint configured is refused when the host starts, not when it runs |
| 7 | Network support needs no declared handlers | The prover changes only for channel-keyed `emit`; record the line count of the `prove.py` diff |
| 8 | Crash recovery | `kill -9` during a flush, then restart: remaining events are delivered, with at most 1 duplicate per kill |

## Steps

Commit after each step, with its scorecard items measured.

### H1: lazy world and key-scoped invariant checks (items 1, 2)

- `Host.load()` reads the whole world on every call. Replace it with a mapping backed by
  SQLite that loads keys on demand and records which keys were written. `ledger.total` is
  `sum(w.bal.values())` in `run.py`, so keep a running total in `meta` (or use SQL `sum`).
- `Interp.run` takes a snapshot with `world.copy()`, which would force a full load. Give
  `run` a way to skip the snapshot when the caller supplies rollback; for the host,
  SQLite rollback is that mechanism. This is a change to `run.py`.
- `broken_invariants` loops over every key. The host should check only the keys the call
  wrote, which matches the prover: `prove.py:308` checks invariants at the keys a def
  writes. Where the prover marked an invariant "unknown" (a write at a computed key), keep
  the full check for that def.
- **Soundness condition:** checking only the written keys is sound only if an invariant
  reads nothing but its own bound key (all of escrow's do). Verify that `wf.invariant_problems`
  enforces this, or add the check.

### H2: named output channels (items 3, 4, 7)

- Syntax proposal, to be measured: a module-level `channel console`. In the body,
  `console.emit (Line x)`. In the spec, `emit console Line x`. With no channel named, the
  channel is `audit`, so existing code and specs don't change.
- Prover: `Sym.audit` becomes a dict from channel to list, with an exact sequence per channel.
  The order across channels is unspecified; state that on the proof page.
- `infer.uses` reports `console`, `net` and so on as separate effects, so the manifest
  separates them.
- Host: the `outbox` table gets a `channel` column, and each channel gets its own sink.
  `console` goes to stdout. `audit` stays as it is now.

### H3: requests as events (items 5, 6)

- Add a `net` channel. A request constructor maps to an endpoint in a host config file
  next to the module, e.g. `rates.host`:
  `Fetch -> GET http://127.0.0.1:PORT/limit/{1}; reply got_limit {1} $.value; fail limit_failed {1}`.
  The host validates the config at startup against the module's emitted constructors and
  defs: unknown names fail, and so does a reply def with the wrong parameter types.
- The host decodes the reply against that path into Int arguments; Kern never sees bytes.
  A timeout, non-2xx status or decode error becomes the `fail` call.
- **Simulation mode:** a `net` sink a test can drive directly (drop, duplicate, reorder,
  delay), so item 5 runs deterministically without sockets. Also add one test against a
  real local `HTTPServer` fixture.
- Resource-labelled effects come from the config: the manifest says
  `net: GET http://127.0.0.1:PORT/limit/*`.

### H4: demo module `examples/limits.k` (item 5)

- An external risk service sets each account's withdrawal limit:
  `request_limit a` sets `pending.get a = 1` and does `emit net Fetch a`.
  `got_limit a value` has `need pending.get a = 1 else Stale` and
  `need value >= 0 else BadValue`, sets `limit` and clears `pending`.
  `limit_failed a` clears `pending`. Reuse escrow's `withdraw` against `limit`.
- **Arithmetic is linear only.** A fetched exchange rate can't multiply an amount. Pick
  external values that are compared or stored, not multiplied.
- Invariants to prove and then attack with the simulation:
  `pending.get a = 0 or pending.get a = 1`, and balances never go negative.

### H5: show what the host assumes on the proof page

- Add a template-only section to `render.py`, never generated by a model. It lists:
  - the manifest (effects and endpoints) and how the host realises each one;
  - the assumptions: at-least-once delivery, reply decoding, serial calls.
- These are shown as **assumed**, not ✔.

### H6: scorecard and write-up

- Fill in the table above in HOST.md, including failures.
- Update CLAUDE.md's known limitations.
- Update `README.md` if the demo loop changes.

## Out of scope for this proof of concept

- Text and records.
- `await` or durable continuations.
- Wasm/WASI sandboxing.
- Declared handlers for observations inside a call.
- Concurrent calls.
- Exactly-once delivery.

## Guardrails

- Don't change escrow semantics to make a host test pass. If the host and the interpreter
  disagree, the host is wrong until shown otherwise.
- Keep `tests/test_host.py` green after every step. New tests go there too, with strict
  xfails for known gaps, as in `test_fundamentals.py`.
- Never add OS access the module can't name. Everything the host does has to show up in
  the manifest.

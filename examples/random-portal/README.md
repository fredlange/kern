# The Random Information Portal

A React page that fetches funny things from public APIs (jokes, puns) and lets visitors vote
on them. The backend is one Kern module behind the Kern host: every button is a call to a
proven definition, every outbound HTTP request is a `net` event the module emits, and every
reply comes back as a new call. Nothing in the app reaches the network or the database except
through Kern.

```
random-portal/
├── portal.k               request, store, fetch_failed, give_up, upvote
├── portal.host            where each request goes, and which call carries the reply
├── portal.why             rationale, anchored to each definition's hash
├── proof-pages/portal.html
├── web/                   the React app (Vite); talks to the host only through /api
├── Dockerfile             builds web/, installs Kern from this checkout, renders the proof page
├── Dockerfile.dockerignore, nginx.conf, entrypoint.sh, docker-compose.yml
```

## Status

`kern check portal.k`: **5 of 5 definitions clean, every guarantee proven, spec strength
56/56** (request 18/18, store 22/22, fetch_failed 7/7, give_up 4/4, upvote 5/5).

**The loop works in Docker. The jokes don't show yet.** Container smoke test, 2026-11-04:
- three real fetches stored joke 162, pun 73 and joke 417, and `pending` went back to 0;
- the vote landed, and a forged `POST /api/store` got 403;
- the state survived `docker restart`;
- `kern.host replay` inside the container: "replay of 7 logged calls matches the database".

| # | Requested from the Kern developers | Status |
|---|---|---|
| R1 | **Text.** `$.field` in `.host` must be an Int and state maps hold Ints | **Open.** Decided 2026-11-04: Text will be built in the Rust toolchain once the parser, type checker, interpreter, prover and then the host match Python. Not in the Python `kern/`. Until then Kern stores only each joke's id at its source, and the page shows "joke #162 at the source", not the joke |
| R2 | `serve` never ran network replies | Fixed in 6b166c5 |
| R3 | invariants aren't proven at literal keys | Known prover gap, planned for the Rust prover; this module uses parameter keys |
| R5 | `serve --bind` | Fixed in 6b166c5 (unused here: nginx proxies to 127.0.0.1) |
| | two `.why` tool bugs | Fixed in 59f7103 |

Since 1423327 the host records an empty genesis world for a new database, so `replay` works
without `--world`. Rechecked in the container: "replay of 5 logged calls matches the database".

## What the proofs say, and what they don't

Proven for all inputs (✔ on the proof page):
- at most one outstanding request per source (`AlreadyPending`); `pending` is always 0 or 1;
- a reply is stored only if it answers the request still pending for that source and slot.
  A late or duplicated reply is refused with `Stale`, and a non-positive id with `BadId`;
- a source stuck pending can be freed only after its 30-second deadline (`NotYet n` gives the
  seconds left);
- votes go only to stored items, by exactly +1, and are never negative.

Not covered (no guarantee, and the page says so where it can):
- **the content.** Kern can't hold text yet (R1), so nothing is proven about what is shown;
- **who votes.** Visitors have no identity, so one person can vote any number of times;
- **who calls the reply definitions.** `store` and `fetch_failed` are ordinary definitions, and
  the slot they must match can be read from `GET /state`. Anyone who can reach the host can
  answer a pending request with any id. The only thing keeping visitors out is `nginx.conf`
  (`/api/store` and `/api/fetch_failed` → 403; checked in the smoke test). Kern has no way yet
  to say "only the host's net path may call this";
- **the APIs.** Whether a joke is funny, or safe, is up to official-joke-api and jokeapi.dev
  (`safe-mode`). The host section of the proof page lists both endpoints as *assumed*.

## Run it

Check and inspect, from the repository root:

```text
kern check examples/random-portal/portal.k
python -m kern.host manifest examples/random-portal/portal.k
```

Docker (the build context is the repository root, because the image installs Kern from this
checkout):

```text
cd examples/random-portal
docker compose up --build          # http://localhost:8000, proof page at /proof.html
```

Development: run `python -m kern.host serve portal.k --db portal.db --port 8080` here, then
`npm install && npm run dev` in `web/` (Vite proxies `/api` to the host).

## HTTP API (what the page calls)

| call | body | outcomes |
|---|---|---|
| `POST /api/request` | `{"src": 1}` (1 = joke, 2 = pun) | 200, 409 `AlreadyPending` / `UnknownSource` |
| `POST /api/upvote` | `{"item": 3}` | 200, 409 `NoSuchItem` |
| `POST /api/give_up` | `{"src": 1}` | 200, 409 `NotPending` / `NotYet n` |
| `GET /api/state` | | the world: `state.ref`, `state.source`, `state.votes`, `state.pending` by key |
| `GET /api/manifest` | | every definition's inferred effects and failures |

`store` and `fetch_failed` are reply calls the host makes. The host serves them over HTTP like
any definition, so nginx blocks them from the public path (403).

The image serves two proof pages: `/proof.html` (`kern check` with the `.why` rationale and spec
strength; the build fails unless every definition is clean) and `/host.html` (`kern.host page`,
which adds the endpoints and the host's assumptions but doesn't read `.why`).

## Findings for the Kern developers (all reported)

- Invariants are checked only at parameter keys. `pending.set 1 1` gives ◐ for
  `pending.get s ∈ {0,1}`, while the same write keyed by `src:Int` gives ✔. A computed key
  (`next.get 0 + 1`) gives `?`. That's why the slot is reserved by `request` and carried in the
  event.
- `kern why` printed the build hash on both sides of its stale message, and `kern anchor`
  silently dropped entries with placeholder anchors. Both fixed in 59f7103.
- `serve` without `--world` created no genesis world, so `replay` couldn't run. Fixed in 1423327.

# Risk limits

Withdrawals limited per account, with the limit fetched from an outside risk service over
HTTP. It shows how a Kern program talks to the network: a request is an event the program
emits, and the reply comes back later as a new call.

```
limits/
├── limits.k                 request_limit, got_limit, limit_failed, withdraw, reset_day, transfer
├── limits.host              where each request goes, and which call carries the reply
└── proof-pages/limits.html
```

`limits.host` maps the `Fetch` event to an HTTP request, and its reply or failure to a call:

```text
Fetch -> GET http://127.0.0.1:9/limit/{1}; reply got_limit {1} {2} $.value; fail limit_failed {1} {2}
```

Point it at a real service by changing the URL. The service returns JSON with an Int `value`.

## Run it

With Kern installed (`pip install -e <kern repo>`), from this directory:

```text
kern check limits.k
python -m kern.host manifest limits.k      # every effect and endpoint this program can reach
python -m kern.host repl limits.k --world "A:500@u1 K:0@u9"
```

In the console you can play the service yourself and type the reply call:

```text
request_limit A
ok
got_limit A 1 300
ok
audit #2: LimitSet A 300 <now>
withdraw A K 200
ok
audit #3: Moved A K 200 <now>
withdraw A K 200
refused OverLimit 100
```

## What it shows

- **The network is an effect.** `request_limit` emits `Fetch` on the `net` channel. The host
  sends the HTTP request only after the call has committed.
- **Replies are calls.** `got_limit` checks that the reply is for the request still pending
  (`need req.get a = n else Stale`), so a late or duplicated reply is refused.
- **A permission manifest.** `manifest` lists everything the program can touch, inferred from
  the code. Nothing else is reachable.

## Status

`kern check limits.k`: 6 of 6 definitions clean, every guarantee proven.

## Limits

- There's no request timeout. A reply that never comes leaves the request pending.
- Delivery is at-least-once, which is why `got_limit` refuses stale replies.

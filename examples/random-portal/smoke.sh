#!/bin/bash
# Acceptance test for the image: build, run, check, restart, replay. Exit 1 on the first failure.
# Needs docker, curl, jq, and internet access (the fetches hit the real APIs).
# Usage, from anywhere: examples/random-portal/smoke.sh [PORT]
# It builds the repository's kern image first (`--target kern`), then the portal image on top of it.
set -u
cd "$(dirname "$0")/../.."
PORT=${1:-18000}
NAME=random-portal-smoke
B=http://localhost:$PORT
fails=0

ok()   { echo "pass  $1"; }
bad()  { echo "FAIL  $1"; fails=$((fails + 1)); }
code() { curl -s -o /dev/null -w "%{http_code}" "$@"; }
post() { curl -s -X POST "$B/api/$1" -d "$2"; }
jget() { jq -r "$1"; }
wait_up() { for _ in $(seq 1 60); do curl -sf "$B/api/state" >/dev/null && return 0; sleep 1; done; return 1; }

docker build -q --target kern -t kern . >/dev/null || { echo "FAIL  kern image build"; exit 1; }
ok "kern image builds (repository Dockerfile, --target kern)"
docker build -q -f examples/random-portal/Dockerfile -t random-portal-smoke . >/dev/null || { echo "FAIL  portal image build"; exit 1; }
ok "portal image builds (includes: every definition clean, or the build fails)"
docker rm -f $NAME >/dev/null 2>&1
docker run -d --name $NAME -p "$PORT:8000" random-portal-smoke >/dev/null
trap 'docker rm -f $NAME >/dev/null 2>&1' EXIT
wait_up || { echo "FAIL  host did not come up"; docker logs $NAME; exit 1; }

for p in / /proof.html /host.html; do
  [ "$(code "$B$p")" = 200 ] && ok "GET $p" || bad "GET $p"
done
curl -s "$B/proof.html" | grep -q "All guarantees proven" && ok "proof page says All guarantees proven" || bad "proof page wording"

r=$(post request '{"src": 1}')
[ "$(echo "$r" | jget .status)" = ok ] && ok "request joke" || bad "request joke: $r"
r=$(post request '{"src": 1}')
[ "$(echo "$r" | jget .refused)" = TooSoon ] && ok "second request within 5 s refused TooSoon" || bad "cooldown: $r"
r=$(post request '{"src": 2}')
[ "$(echo "$r" | jget .status)" = ok ] && ok "request pun" || bad "request pun: $r"

s=$(curl -s "$B/api/state")
[ "$(echo "$s" | jget '.state.pending["1"] + .state.pending["2"]')" = 0 ] \
  && ok "replies ran: nothing pending" || bad "still pending: $s"
n=$(echo "$s" | jget '[(.state.ref // {})[] | select(. > 0)] | length')
[ "$n" -ge 1 ] && ok "$n item(s) stored from the real APIs" || bad "no items stored (APIs down?): $s"

r=$(post upvote '{"item": 1}')
[ "$(echo "$r" | jget .status)" = ok ] && ok "upvote item 1" || bad "upvote: $r"
r=$(post upvote '{"item": 999}')
[ "$(echo "$r" | jget .refused)" = NoSuchItem ] && ok "upvote missing item refused NoSuchItem" || bad "upvote 999: $r"

[ "$(code -X POST "$B/api/store" -d '{"src": 1, "slot": 1, "id": 7}')" = 403 ] && ok "forged store blocked (403)" || bad "store reachable"
[ "$(code -X POST "$B/api/fetch_failed" -d '{"src": 1, "slot": 1}')" = 403 ] && ok "forged fetch_failed blocked (403)" || bad "fetch_failed reachable"

before=$(curl -s "$B/api/state")
docker restart $NAME >/dev/null
wait_up || bad "host did not come back after restart"
[ "$(curl -s "$B/api/state")" = "$before" ] && ok "state survives restart" || bad "state changed across restart"

r=$(docker exec $NAME kern host replay /app/portal.k --db /data/portal.db)
echo "$r" | grep -q "matches the database" && ok "$r" || bad "replay: $r"

echo
[ $fails = 0 ] && echo "smoke test: all checks passed" || echo "smoke test: $fails check(s) failed"
exit $((fails > 0))

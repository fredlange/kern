#!/bin/bash
# Two processes: the Kern host (proof-gated, refuses to start if anything is not clean or the
# endpoints don't match the module) and nginx. If either one exits, the container exits.
set -e
mkdir -p /data
kern host serve /app/portal.k --db /data/portal.db --port 8080 &
nginx -g 'daemon off;' &
wait -n
exit 1

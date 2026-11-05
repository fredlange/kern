#!/bin/bash
# Two processes: the Kern host (proof-gated, refuses to start if anything is not clean or the
# endpoints don't match the module) and nginx. If either one exits, the container exits.
set -e
: "${KERN_HOST:?set KERN_HOST to the host command (python -m kern.host, or kern host)}"
mkdir -p /data
$KERN_HOST serve /app/portal.k --db /data/portal.db --port 8080 &
nginx -g 'daemon off;' &
wait -n
exit 1

#!/usr/bin/env bash
# Full portability run for one flavor: create cluster + deploy + smoke (up.sh), k6 load suite,
# pod-kill chaos, then tear the cluster down (KEEP=1 keeps it).
#   scripts/k8s/e2e.sh k3d|kind
set -euo pipefail
FLAVOR=${1:?usage: e2e.sh k3d|kind}
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
t0=$(date +%s)
"$ROOT/scripts/k8s/up.sh" "$FLAVOR"
# shellcheck disable=SC1090
source "$ROOT/data/k8s/$FLAVOR.env"
CLUSTER=$FLAVOR "$ROOT/tests/load/run.sh" "$FORGE_URL" "$FORGE_TOKEN"
rc=0
"$ROOT/tests/chaos/pod-kill.sh" "$FLAVOR" || rc=$?
echo "==> e2e $FLAVOR finished in $(( $(date +%s) - t0 ))s (chaos rc=$rc)"
[ "${KEEP:-0}" = 1 ] || "$ROOT/scripts/k8s/down.sh" "$FLAVOR"
exit $rc

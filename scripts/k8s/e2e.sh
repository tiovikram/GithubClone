#!/usr/bin/env bash
# Full portability run for one flavor: create cluster + deploy + smoke (up.sh), full smoke
# (Actions on the in-cluster runner, OCI + npm registries: smoke-full.sh), k6 load suite,
# pod-kill chaos, runner-kill chaos, registry (chunked upload) chaos, then tear the cluster
# down (KEEP=1 keeps it). SKIP_LOAD=1 / SKIP_CHAOS=1 skip those parts.
#   scripts/k8s/e2e.sh k3d|kind
set -euo pipefail
FLAVOR=${1:?usage: e2e.sh k3d|kind}
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
t0=$(date +%s)
"$ROOT/scripts/k8s/up.sh" "$FLAVOR"
"$ROOT/scripts/k8s/smoke-full.sh" "$FLAVOR"
# shellcheck disable=SC1090
source "$ROOT/data/k8s/$FLAVOR.env"
[ "${SKIP_LOAD:-0}" = 1 ] || CLUSTER=$FLAVOR "$ROOT/tests/load/run.sh" "$FORGE_URL" "$FORGE_TOKEN"
rc=0
if [ "${SKIP_CHAOS:-0}" != 1 ]; then
  "$ROOT/tests/chaos/pod-kill.sh" "$FLAVOR" || rc=$?
  "$ROOT/tests/chaos/runner-kill.sh" "$FLAVOR" || rc=$?
  "$ROOT/tests/chaos/registry-kill.sh" "$FLAVOR" || rc=$?
fi
echo "==> e2e $FLAVOR finished in $(( $(date +%s) - t0 ))s (chaos rc=$rc)"
[ "${KEEP:-0}" = 1 ] || "$ROOT/scripts/k8s/down.sh" "$FLAVOR"
exit $rc

#!/usr/bin/env bash
# Deletes the local cluster created by scripts/k8s/up.sh (and its volumes).
#   scripts/k8s/down.sh k3d|kind
set -euo pipefail
FLAVOR=${1:?usage: down.sh k3d|kind}
CLUSTER=${CLUSTER:-forge}
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
case "$FLAVOR" in
  k3d) k3d cluster delete "$CLUSTER" ;;
  kind) kind delete cluster --name "$CLUSTER" ;;
  *) echo "unknown flavor $FLAVOR (k3d|kind)" >&2; exit 2 ;;
esac
rm -f "$ROOT/data/k8s/$FLAVOR.env"
echo "deleted $FLAVOR cluster $CLUSTER"

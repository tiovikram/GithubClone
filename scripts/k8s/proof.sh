#!/usr/bin/env bash
# Prints the portability evidence for a running cluster: server version, host image IDs,
# rendered-manifest hash, and the image digests the cluster's containers actually run.
#   scripts/k8s/proof.sh k3d|kind      (diff the output of both flavors: only versions differ)
set -euo pipefail
FLAVOR=${1:?usage: proof.sh k3d|kind}
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
CTX=$FLAVOR-${CLUSTER:-forge}
echo "flavor: $FLAVOR ($(kubectl --context "$CTX" version -o json | jq -r .serverVersion.gitVersion), $(kubectl --context "$CTX" get nodes --no-headers | wc -l | tr -d ' ') nodes)"
for i in forge:dev forge-runner:dev; do echo "host image $i: $(docker image inspect "$i" --format '{{.Id}}')"; done
echo "rendered manifests sha256: $(kubectl kustomize "$ROOT/deploy/overlays/$FLAVOR" | shasum -a 256 | cut -d' ' -f1)"
echo "running images (image -> imageID):"
kubectl --context "$CTX" -n forge get pods -o jsonpath='{range .items[*]}{range .status.initContainerStatuses[*]}{.image} {.imageID}{"\n"}{end}{range .status.containerStatuses[*]}{.image} {.imageID}{"\n"}{end}{end}' \
  | sed 's#^docker.io/library/##' | sort -u | sed 's/^/  /'
# The imageID above is runtime-specific (k3s reports the config digest, kind's
# `load image-archive` an import@<manifest digest>). The image config digest and the layer
# diff_ids are what identify the content: compare these between flavors.
case $FLAVOR in
  k3d) NODE=k3d-${CLUSTER:-forge}-agent-0 ;;
  kind) NODE=${CLUSTER:-forge}-worker ;;
esac
echo "image content on node $NODE (config digest, sha256 of the layer diff_ids list):"
for img in forge:dev forge-runner:dev postgres:16 cgr.dev/chainguard/minio:latest cgr.dev/chainguard/minio-client:latest; do
  ref=$img; case $img in */*) ;; *) ref=docker.io/library/$img ;; esac
  j=$(docker exec "$NODE" crictl inspecti -o json "$ref")
  echo "  $img config $(echo "$j" | jq -r .status.id) layers $(echo "$j" | jq -c .info.imageSpec.rootfs.diff_ids | shasum -a 256 | cut -c1-16)"
done

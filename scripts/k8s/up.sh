#!/usr/bin/env bash
# Creates a local Kubernetes cluster (k3d or kind), builds the forge image, imports all images
# (no registry), applies deploy/overlays/<flavor>, waits for rollout, creates an admin user +
# token and runs a smoke test (REST API + git clone/push through the cluster).
#
#   scripts/k8s/up.sh k3d|kind
#
# Only the "create cluster" step differs between flavors; images and manifests are identical.
# Env: CLUSTER (default forge), FORGE_HOST_PORT (default 8080; must match FORGE_BASE_URL in
# deploy/base/kustomization.yaml), SKIP_BUILD=1 to reuse an existing forge:dev image.
set -euo pipefail
FLAVOR=${1:?usage: up.sh k3d|kind}
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
CLUSTER=${CLUSTER:-forge}
PORT=${FORGE_HOST_PORT:-8080}
NODE_PORT=30080
IMAGE=forge:dev
DEP_IMAGES=(postgres:16 cgr.dev/chainguard/minio:latest cgr.dev/chainguard/minio-client:latest)
ARCH=$(docker version --format '{{.Server.Arch}}')
STATE="$ROOT/data/k8s"
mkdir -p "$STATE"
export GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_GLOBAL=/dev/null GIT_TERMINAL_PROMPT=0

log() { printf '\n\033[1m==> %s\033[0m\n' "$*"; }
t0=$(date +%s)
since() { echo "$(( $(date +%s) - t0 ))s"; }

# ---------------------------------------------------------------- 1. cluster (flavor-specific)
case "$FLAVOR" in
  k3d)
    CTX="k3d-$CLUSTER"
    if ! k3d cluster list "$CLUSTER" >/dev/null 2>&1; then
      log "creating k3d cluster $CLUSTER (1 server + 2 agents, host :$PORT -> NodePort $NODE_PORT)"
      # traefik disabled: we expose forge through a NodePort on every flavor.
      k3d cluster create "$CLUSTER" --servers 1 --agents 2 \
        -p "$PORT:$NODE_PORT@server:0" \
        --k3s-arg "--disable=traefik@server:*" \
        --wait --timeout 180s
    fi
    ;;
  kind)
    CTX="kind-$CLUSTER"
    if ! kind get clusters 2>/dev/null | grep -qx "$CLUSTER"; then
      log "creating kind cluster $CLUSTER (1 control-plane + 2 workers, host :$PORT -> NodePort $NODE_PORT)"
      cat > "$STATE/kind-config.yaml" <<EOF
kind: Cluster
apiVersion: kind.x-k8s.io/v1alpha4
nodes:
  - role: control-plane
    extraPortMappings:
      - {containerPort: $NODE_PORT, hostPort: $PORT, listenAddress: "127.0.0.1", protocol: TCP}
  - role: worker
  - role: worker
EOF
      kind create cluster --name "$CLUSTER" --config "$STATE/kind-config.yaml" --wait 180s
      # kind ships no metrics-server (k3s bundles one); the HPA needs it. Cluster add-on, not
      # an app manifest change.
      log "installing metrics-server (kind only; k3d bundles it)"
      kubectl --context "$CTX" apply -f https://github.com/kubernetes-sigs/metrics-server/releases/latest/download/components.yaml
      kubectl --context "$CTX" -n kube-system patch deployment metrics-server --type=json \
        -p '[{"op":"add","path":"/spec/template/spec/containers/0/args/-","value":"--kubelet-insecure-tls"}]'
    fi
    ;;
  *) echo "unknown flavor $FLAVOR (k3d|kind)" >&2; exit 2 ;;
esac
K="kubectl --context $CTX"
log "cluster ready ($(since))"

# ---------------------------------------------------------------- 2. images (no registry)
if [ "${SKIP_BUILD:-0}" != 1 ]; then
  log "building $IMAGE (linux/$ARCH)"
  docker build --provenance=false --sbom=false -t "$IMAGE" "$ROOT"
fi
echo "$IMAGE id: $(docker image inspect "$IMAGE" --format '{{.Id}}')"
log "importing images into $FLAVOR"
for img in "${DEP_IMAGES[@]}"; do
  docker image inspect "$img" >/dev/null 2>&1 || docker pull -q --platform "linux/$ARCH" "$img"
done
# Single-platform archives: Docker Desktop's containerd store keeps multi-arch indexes whose
# other-platform blobs are absent, which `kind load docker-image` / `k3d image import` choke on.
TARS=()
for img in "$IMAGE" "${DEP_IMAGES[@]}"; do
  tar="$STATE/$(echo "$img" | tr '/:' '__').tar"
  docker save --platform "linux/$ARCH" -o "$tar" "$img"
  TARS+=("$tar")
done
case "$FLAVOR" in
  k3d) k3d image import -c "$CLUSTER" "${TARS[@]}" ;;
  kind) for t in "${TARS[@]}"; do kind load image-archive --name "$CLUSTER" "$t"; done ;;
esac
rm -f "${TARS[@]}"
log "images imported ($(since))"

# ---------------------------------------------------------------- 3. deploy (identical manifests)
existed=0
$K -n forge get deploy forge >/dev/null 2>&1 && existed=1
log "kubectl apply -k deploy/overlays/$FLAVOR"
$K apply -k "$ROOT/deploy/overlays/$FLAVOR"
# Same tag, new image content: restart so pods pick up the freshly imported image.
[ $existed = 1 ] && $K -n forge rollout restart deploy/forge
$K -n forge rollout status statefulset/postgres --timeout=300s
$K -n forge rollout status statefulset/minio --timeout=300s
$K -n forge wait --for=condition=complete job/minio-create-bucket --timeout=300s
$K -n forge rollout status deploy/forge --timeout=300s
log "rollout complete ($(since))"
$K -n forge get pods -o wide

# ---------------------------------------------------------------- 4. admin user + token
BASE="http://localhost:$PORT"
for _ in $(seq 1 60); do curl -sf "$BASE/readyz" >/dev/null && break; sleep 1; done
ADMIN_PW=${FORGE_ADMIN_PASSWORD:-admin-password-123}
$K -n forge exec deploy/forge -c forge -- forge admin create-user --login admin --password "$ADMIN_PW" --admin 2>/dev/null \
  || echo "(admin user already exists)"
TOKEN=$($K -n forge exec deploy/forge -c forge -- forge admin create-token --login admin --name "up-$FLAVOR" 2>/dev/null | tail -1)
printf 'FORGE_URL=%s\nFORGE_TOKEN=%s\nKUBE_CONTEXT=%s\n' "$BASE" "$TOKEN" "$CTX" > "$STATE/$FLAVOR.env"
echo "token written to data/k8s/$FLAVOR.env"

# ---------------------------------------------------------------- 5. smoke test
log "smoke test against $BASE"
fail() { echo "SMOKE FAIL: $*" >&2; exit 1; }
api() { curl -sS -H "Authorization: token $TOKEN" -H 'Accept: application/vnd.github+json' "$@"; }
login=$(api "$BASE/api/v3/user" | jq -r .login)
[ "$login" = admin ] || fail "GET /user returned '$login'"
echo "GET /api/v3/user -> $login"
repo="smoke-$(date +%s)"
code=$(api -o /dev/null -w '%{http_code}' -X POST "$BASE/api/v3/user/repos" -d "{\"name\":\"$repo\"}")
[ "$code" = 201 ] || fail "POST /user/repos -> $code"
echo "POST /api/v3/user/repos -> 201 ($repo)"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
url="http://x-access-token:$TOKEN@localhost:$PORT/admin/$repo.git"
git init -q -b main "$work/src"
for i in 1 2 3; do
  echo "smoke $i $(date +%s)-$RANDOM" > "$work/src/file$i.txt"
  git -C "$work/src" add . && git -C "$work/src" -c user.name=smoke -c user.email=smoke@forge.local commit -qm "smoke $i"
done
git -C "$work/src" push -q "$url" main || fail "git push"
echo "git push -> ok ($(git -C "$work/src" rev-parse --short HEAD))"
# Clone several times so different pods (cold caches) serve it.
for i in 1 2 3; do
  rm -rf "$work/clone"
  git clone -q "$url" "$work/clone" || fail "git clone #$i"
  [ "$(git -C "$work/clone" rev-parse HEAD)" = "$(git -C "$work/src" rev-parse HEAD)" ] || fail "clone #$i HEAD mismatch"
  git -C "$work/clone" fsck --strict --no-progress >/dev/null 2>&1 || fail "fsck clone #$i"
done
echo "git clone x3 -> HEAD matches, fsck clean"
contents=$(api "$BASE/api/v3/repos/admin/$repo" | jq -r .full_name)
[ "$contents" = "admin/$repo" ] || fail "GET /repos/admin/$repo"
echo "GET /api/v3/repos/admin/$repo -> ok"
log "forge is up on $FLAVOR at $BASE  (total $(since))"
echo "  export FORGE_URL=$BASE FORGE_TOKEN=$TOKEN"
echo "  load test:  tests/load/run.sh $BASE \$FORGE_TOKEN"
echo "  chaos test: tests/chaos/pod-kill.sh $FLAVOR"

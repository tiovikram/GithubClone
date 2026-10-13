#!/usr/bin/env bash
# Benchmark cluster for the forge-vs-Forgejo baseline. Same k3d topology as scripts/k8s/up.sh
# (1 server + 2 agents, traefik off, host port -> NodePort 30080), but its own cluster name and
# host port so it never collides with a dev cluster. Images are imported, never pulled from a
# registry by the cluster.
#   k8s.sh up                      create cluster "bench" + import all images
#   k8s.sh deploy forge [N]        apply tests/baseline/forge, scale to N (default 1), admin+token
#   k8s.sh deploy forgejo          apply tests/baseline/forgejo (1 replica), admin+token
#   k8s.sh scale forge N           change forge's replica count and wait
#   k8s.sh down                    delete the cluster
# Writes data/bench/<app>.env: APP BASE TOKEN OWNER NS CTX REPLICAS
set -euo pipefail
HERE=$(cd "$(dirname "$0")" && pwd); ROOT=$(cd "$HERE/../.." && pwd)
CLUSTER=${BENCH_CLUSTER:-bench}; PORT=${BENCH_PORT:-8088}
CTX="k3d-$CLUSTER"; K="kubectl --context $CTX"
STATE="$ROOT/data/bench"; mkdir -p "$STATE"
FORGEJO_IMAGE=codeberg.org/forgejo/forgejo:16.0.6
IMAGES=(forge:dev postgres:16 cgr.dev/chainguard/minio:latest cgr.dev/chainguard/minio-client:latest "$FORGEJO_IMAGE")
ARCH=$(docker version --format '{{.Server.Arch}}')
OWNER=bench; PW=bench-password-123
log() { printf '\n\033[1m==> %s\033[0m\n' "$*"; }

wait_ready() { # ns deploy replicas
  for _ in $(seq 1 300); do
    [ "$($K -n "$1" get deploy "$2" -o jsonpath='{.status.readyReplicas}' 2>/dev/null || echo 0)" = "$3" ] \
      && [ "$($K -n "$1" get pods -l "app.kubernetes.io/name=$2" --no-headers 2>/dev/null | wc -l | tr -d ' ')" = "$3" ] && return 0
    sleep 1
  done
  echo "$2 did not reach $3 ready replicas" >&2; return 1
}
write_env() { # app ns replicas token
  printf 'APP=%s\nBASE=http://localhost:%s\nTOKEN=%s\nOWNER=%s\nNS=%s\nCTX=%s\nREPLICAS=%s\n' \
    "$1" "$PORT" "$4" "$OWNER" "$2" "$CTX" "$3" > "$STATE/$1.env"
  echo "wrote data/bench/$1.env"
}

case "${1:-}" in
up)
  if ! k3d cluster list "$CLUSTER" >/dev/null 2>&1; then
    log "creating k3d cluster $CLUSTER (1 server + 2 agents, host :$PORT -> NodePort 30080)"
    k3d cluster create "$CLUSTER" --servers 1 --agents 2 -p "$PORT:30080@server:0" \
      --k3s-arg "--disable=traefik@server:*" --wait --timeout 180s
  fi
  log "importing images (linux/$ARCH)"
  tars=()
  for img in "${IMAGES[@]}"; do
    docker image inspect "$img" >/dev/null 2>&1 || docker pull -q --platform "linux/$ARCH" "$img"
    t="$STATE/$(echo "$img" | tr '/:' '__').tar"; docker save --platform "linux/$ARCH" -o "$t" "$img"; tars+=("$t")
  done
  k3d image import -c "$CLUSTER" "${tars[@]}"
  rm -f "${tars[@]}"
  ;;
deploy)
  app=${2:?deploy forge|forgejo}
  case $app in
  forge)
    n=${3:-1}
    $K apply -k "$HERE/forge"
    $K -n forge rollout status statefulset/postgres --timeout=300s
    $K -n forge rollout status statefulset/minio --timeout=300s
    $K -n forge wait --for=condition=complete job/minio-create-bucket --timeout=300s
    $K -n forge scale deploy/forge --replicas="$n"
    wait_ready forge forge "$n"
    for _ in $(seq 1 60); do curl -sf "localhost:$PORT/readyz" >/dev/null && break; sleep 1; done
    $K -n forge exec deploy/forge -c forge -- forge admin create-user --login $OWNER --password $PW --admin >/dev/null 2>&1 || true
    tok=$($K -n forge exec deploy/forge -c forge -- forge admin create-token --login $OWNER --name "bench-$RANDOM" | tail -1)
    write_env forge forge "$n" "$tok"
    ;;
  forgejo)
    $K apply -k "$HERE/forgejo"
    $K -n forgejo rollout status statefulset/postgres --timeout=300s
    wait_ready forgejo forgejo 1
    for _ in $(seq 1 60); do curl -sf "localhost:$PORT/api/healthz" >/dev/null && break; sleep 1; done
    fj() { $K -n forgejo exec deploy/forgejo -c forgejo -- su-exec git forgejo "$@"; }
    fj admin user create --admin --username $OWNER --password $PW --email bench@example.com --must-change-password=false >/dev/null 2>&1 || true
    tok=$(fj admin user generate-access-token -u $OWNER -t "bench-$RANDOM" --scopes all --raw | tail -1)
    write_env forgejo forgejo 1 "$tok"
    ;;
  *) echo "unknown app $app" >&2; exit 2;;
  esac
  $K -n "$app" get pods -o wide
  ;;
scale)
  n=${3:?scale forge N}
  $K -n forge scale deploy/forge --replicas="$n"
  wait_ready forge forge "$n"
  sed -i.bak "s/^REPLICAS=.*/REPLICAS=$n/" "$STATE/forge.env" && rm -f "$STATE/forge.env.bak"
  $K -n forge get pods -o wide
  ;;
down)
  k3d cluster delete "$CLUSTER"; rm -f "$STATE"/forge.env "$STATE"/forgejo.env ;;
*) sed -n 2,13p "$0"; exit 2 ;;
esac

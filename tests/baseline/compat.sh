#!/usr/bin/env bash
# GitHub-API compatibility baseline: forge vs Forgejo, same harnesses, same machine.
#   tests/baseline/compat.sh [step...]     steps: up conformance gh git down   (default: all)
#
# Both servers run from tests/baseline/compat/docker-compose.yml (one Postgres 16, forge:dev,
# Forgejo pinned image) - no Kubernetes needed for correctness tests.
#   conformance  tools/conformance (GHES 3.22 spec, --org + --collaborator) against
#                forge /api/v3, Forgejo /api/v1 (its GitHub-like API) and Forgejo /api/v3 (what a
#                GitHub client pointed at the host gets)
#   gh           tests/gh/run.sh (GH_SUITE_EXTERNAL=1) against both as an enterprise host, gh over
#                http_unix_socket through tests/baseline/sockproxy.py, git via url.insteadOf
#   git          tests/baseline/git-smoke.sh against both (stock git smart HTTP)
# Results: tests/baseline/results/compat/
set -euo pipefail
HERE=$(cd "$(dirname "$0")" && pwd); ROOT=$(cd "$HERE/../.." && pwd)
OUT="$HERE/results/compat"; mkdir -p "$OUT"
STATE="$ROOT/data/bench"; mkdir -p "$STATE"
FPORT=3310; JPORT=3320
export FORGE_PORT=$FPORT FORGEJO_PORT=$JPORT
export GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_GLOBAL=/dev/null GIT_TERMINAL_PROMPT=0
DC="docker compose -f $HERE/compat/docker-compose.yml"
STEPS=${*:-"up conformance gh git down"}
log() { printf '\n\033[1m==> %s\033[0m\n' "$*"; }

# up MODE: MODE=local (URLs = http://localhost:port, for conformance/git) | gh (https host names)
up() {
  if [ "$1" = gh ]; then
    export FORGE_BASE_URL=https://forge.localhost FORGEJO_ROOT_URL=https://forgejo.localhost/
  else
    export FORGE_BASE_URL=http://localhost:$FPORT FORGEJO_ROOT_URL=http://localhost:$JPORT/
  fi
  $DC up -d --wait >/dev/null
  for _ in $(seq 1 90); do
    curl -sf "localhost:$FPORT/readyz" >/dev/null && curl -sf "localhost:$JPORT/api/healthz" >/dev/null && return 0
    sleep 1
  done
  echo "servers did not become healthy" >&2; exit 1
}
fj() { $DC exec -T --user 1000:1000 forgejo forgejo "$@"; }
fg() { $DC exec -T forge forge "$@"; }
# users LOGIN... : create on both (idempotent), tokens into $STATE/compat.env
users() {
  touch "$STATE/compat.env"
  for u in "$@"; do
    grep -q "^FORGE_TOKEN_${u//-/_}=" "$STATE/compat.env" && continue
    # The collaborator must be a regular user: conformance checks that it is refused what only
    # admins may do (e.g. bypassing push protection), and a site admin may do everything.
    adm=--admin; [ "$u" = collab ] && adm=""
    fg admin create-user --login "$u" --password "pw-$u-123456" --email "$u@example.com" $adm >/dev/null
    fj admin user create $adm --username "$u" --password "pw-$u-123456" --email "$u@example.com" --must-change-password=false >/dev/null
    echo "FORGE_TOKEN_${u//-/_}=$(fg admin create-token --login "$u" | tail -1)" >> "$STATE/compat.env"
    echo "FORGEJO_TOKEN_${u//-/_}=$(fj admin user generate-access-token -u "$u" -t "bench-$RANDOM" --scopes all --raw | tail -1)" >> "$STATE/compat.env"
  done
  # shellcheck disable=SC1091
  source "$STATE/compat.env"
}

for step in $STEPS; do case $step in
up)
  log "starting compat test bed (forge:dev + $($DC config --images | grep forgejo))"
  up local
  users conf collab
  # org owned by conf (enables the harness' org scenarios) - each server's own org-create API
  curl -sS -o /dev/null -H "Authorization: token $FORGE_TOKEN_conf" -X POST "localhost:$FPORT/api/v3/admin/organizations" \
    -d '{"login":"conforg","admin":"conf"}' || true
  curl -sS -o /dev/null -H "Authorization: token $FORGEJO_TOKEN_conf" -H 'Content-Type: application/json' \
    -X POST "localhost:$JPORT/api/v1/orgs" -d '{"username":"conforg"}' || true
  ;;
conformance)
  up local; users conf collab
  ( cd "$ROOT/tools/conformance"
    [ -d node_modules ] || npm install --silent
    [ -f spec/ghes-3.22.json ] || node fetch-spec.mjs
    # The runner scenarios start a local `forge runner` (FORGE_BIN, default target/debug/forge)
    # against the server; it runs jobs with the host shell (Forgejo's API does not get that far).
    export FORGE_BIN=${FORGE_BIN:-$ROOT/target/debug/forge}
    for target in "forge-v3 http://localhost:$FPORT/api/v3 $FORGE_TOKEN_conf $FORGE_TOKEN_collab" \
                  "forgejo-v1 http://localhost:$JPORT/api/v1 $FORGEJO_TOKEN_conf $FORGEJO_TOKEN_collab" \
                  "forgejo-v3 http://localhost:$JPORT/api/v3 $FORGEJO_TOKEN_conf $FORGEJO_TOKEN_collab"; do
      set -- $target
      log "conformance: $1 ($2)"
      node run.mjs --base "$2" --token "$3" --org conforg --collaborator collab --collaborator-token "$4" \
        --report "$OUT/conformance-$1.json" --md "$OUT/conformance-$1.md" 2>&1 | tail -25 | tee "$OUT/conformance-$1.txt"
    done )
  ;;
gh)
  up gh
  # fresh users per run (the suite creates fixed repo names: demo, readme, apirepo)
  sfx=$(openssl rand -hex 3); A="alice-$sfx"; B="bob-$sfx"
  users "$A" "$B"
  tok() { eval "echo \$${1}_TOKEN_${2//-/_}"; }
  for target in "forge forge.localhost $FPORT $(tok FORGE "$A") $(tok FORGE "$B")" \
                "forgejo forgejo.localhost $JPORT $(tok FORGEJO "$A") $(tok FORGEJO "$B")"; do
    set -- $target
    sock="/tmp/bench-gh-$1.sock"
    python3 "$HERE/sockproxy.py" "$sock" 127.0.0.1 "$3" & ppid=$!
    for _ in $(seq 1 50); do [ -S "$sock" ] && break; sleep 0.1; done
    log "gh suite against $1 ($2, socket -> 127.0.0.1:$3)"
    GH_SUITE_EXTERNAL=1 GH_SUITE_HOST="$2" GH_SUITE_SOCK="$sock" FORGE_GH_PORT="$3" \
      GH_SUITE_ALICE="$A" GH_SUITE_BOB="$B" GH_SUITE_TOKEN="$4" GH_SUITE_BOB_TOKEN="$5" \
      GH_SUITE_WORK="$STATE/gh-$1" GH_SUITE_REPORT="$OUT/gh-$1.md" \
      bash "$ROOT/tests/gh/run.sh" > "$OUT/gh-$1.txt" 2>&1 || true
    tail -2 "$OUT/gh-$1.txt" | head -1
    kill $ppid 2>/dev/null; rm -f "$sock"
  done
  ;;
git)
  up local; users conf
  for target in "forge http://localhost:$FPORT http://localhost:$FPORT/api/v3 $FORGE_TOKEN_conf" \
                "forgejo http://localhost:$JPORT http://localhost:$JPORT/api/v1 $FORGEJO_TOKEN_conf"; do
    set -- $target
    log "git smart HTTP smoke: $1"
    bash "$HERE/git-smoke.sh" "$2" "$3" conf "$4" | tee "$OUT/git-$1.txt" || true
  done
  ;;
down)
  $DC down -v >/dev/null; rm -f "$STATE/compat.env" "$STATE/compat.env.bak"; log "compat test bed removed" ;;
*) echo "unknown step $step" >&2; exit 2 ;;
esac; done

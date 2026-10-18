#!/usr/bin/env bash
# End-to-end smoke test of a cluster created by scripts/k8s/up.sh, through the NodePort only:
#   1. git push + clone over smart HTTP (fsck --strict, HEAD match)
#   2. GitHub Actions: a pushed workflow (`on: push`; bash, actions/checkout, upload-artifact)
#      runs on an in-cluster forge-runner pod and completes `success`; the artifact is
#      downloaded through the REST API and its content checked
#   3. OCI registry: `docker login/push/pull localhost:<port>/<owner>/<image>` of a tiny image
#      built FROM scratch (no pulls); config digest identical after the round trip
#   4. npm registry: `npm publish` of a scoped package, then `npm install` + require() it
#   scripts/k8s/smoke-full.sh k3d|kind           (reads data/k8s/<flavor>.env)
# Never touches the user's credentials: git runs with GIT_CONFIG_GLOBAL=/dev/null, docker with
# a throwaway DOCKER_CONFIG, npm with a throwaway userconfig and cache.
set -euo pipefail
FLAVOR=${1:?usage: smoke-full.sh k3d|kind}
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
# shellcheck disable=SC1090
source "$ROOT/data/k8s/$FLAVOR.env"
BASE=$FORGE_URL; TOKEN=$FORGE_TOKEN
HOSTPORT=${BASE#http://}
K="kubectl --context $KUBE_CONTEXT -n forge"
export GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_GLOBAL=/dev/null GIT_TERMINAL_PROMPT=0
RUN_TIMEOUT=${RUN_TIMEOUT:-300}
W=$(mktemp -d); trap 'rm -rf "$W"' EXIT
STAMP=$(date +%s)
t0=$(date +%s); since() { echo "$(( $(date +%s) - t0 ))s"; }
log() { printf '\n\033[1m==> %s\033[0m\n' "$*"; }
ok() { echo "   ok: $*"; }
fail() { echo "SMOKE FAIL: $*" >&2; exit 1; }
api() { curl -sS -H "Authorization: token $TOKEN" -H 'Accept: application/vnd.github+json' "$@"; }
OWNER=$(api "$BASE/api/v3/user" | jq -r .login)
[ -n "$OWNER" ] && [ "$OWNER" != null ] || fail "token rejected"
URLP="http://x-access-token:$TOKEN@$HOSTPORT/$OWNER"
gitc() { git -c user.name=smoke -c user.email=smoke@forge.local "$@"; }

# ------------------------------------------------------------------ 1+2. git + Actions
log "git push + Actions workflow (on: push) -> forge-runner"
$K rollout status deploy/forge-runner --timeout=180s >/dev/null
runners=$($K get pods -l app.kubernetes.io/name=forge-runner --field-selector=status.phase=Running -o name | wc -l | tr -d ' ')
ok "$runners runner pod(s) running"
repo="smoke-actions-$STAMP"
code=$(api -o /dev/null -w '%{http_code}' -X POST "$BASE/api/v3/user/repos" -d "{\"name\":\"$repo\"}")
[ "$code" = 201 ] || fail "create repo -> $code"
src="$W/src"; git init -q -b main "$src"
printf '# %s\n\nsmoke %s\n' "$repo" "$RANDOM$RANDOM" > "$src/README.md"
mkdir -p "$src/.github/workflows"
cat > "$src/.github/workflows/ci.yml" <<'YAML'
name: smoke
on: push
jobs:
  build:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - name: bash + runner tools
        shell: bash
        run: |
          set -euo pipefail
          echo "bash $BASH_VERSION on $(uname -sm) as uid $(id -u), runner $RUNNER_NAME"
          test -f README.md
          [ "$(git rev-parse HEAD)" = "$GITHUB_SHA" ]
          mkdir -p out
          echo "$GITHUB_SHA" > out/sha.txt
          sha256sum README.md > out/sums.txt
          tar czf out/src.tgz README.md .github
          tar tzf out/src.tgz >/dev/null
          curl --version | head -1
          jq --version
          echo "steps ok" >> "$GITHUB_STEP_SUMMARY"
      - uses: actions/upload-artifact@v4
        with:
          name: smoke-out
          path: out/
YAML
git -C "$src" add -A && gitc -C "$src" commit -qm "smoke workflow"
sha=$(git -C "$src" rev-parse HEAD)
git -C "$src" push -q "$URLP/$repo.git" main || fail "git push"
ok "git push ($sha)"
for i in 1 2; do
  rm -rf "$W/clone"; git clone -q "$URLP/$repo.git" "$W/clone" || fail "git clone #$i"
  [ "$(git -C "$W/clone" rev-parse HEAD)" = "$sha" ] || fail "clone #$i HEAD mismatch"
  git -C "$W/clone" fsck --strict --no-progress >/dev/null 2>&1 || fail "fsck clone #$i"
done
ok "git clone x2: HEAD matches, fsck --strict clean"

run_id=""; status=""; concl=""
deadline=$(( $(date +%s) + RUN_TIMEOUT ))
while [ "$(date +%s)" -lt $deadline ]; do
  r=$(api "$BASE/api/v3/repos/$OWNER/$repo/actions/runs?head_sha=$sha")
  run_id=$(echo "$r" | jq -r '.workflow_runs[0].id // empty')
  status=$(echo "$r" | jq -r '.workflow_runs[0].status // empty')
  concl=$(echo "$r" | jq -r '.workflow_runs[0].conclusion // empty')
  [ "$status" = completed ] && break
  sleep 2
done
[ -n "$run_id" ] || fail "no workflow run created for $sha"
jobs=$(api "$BASE/api/v3/repos/$OWNER/$repo/actions/runs/$run_id/jobs")
runner_name=$(echo "$jobs" | jq -r '.jobs[0].runner_name')
if [ "$status" != completed ] || [ "$concl" != success ]; then
  echo "$jobs" | jq '.jobs[] | {name, status, conclusion, runner_name, steps: [.steps[] | {name, conclusion}]}' >&2
  jid=$(echo "$jobs" | jq -r '.jobs[0].id')
  api -L "$BASE/api/v3/repos/$OWNER/$repo/actions/jobs/$jid/logs" >&2 || true
  fail "run $run_id: status=$status conclusion=$concl"
fi
ok "run $run_id completed: success on runner $runner_name ($(echo "$jobs" | jq -r '[.jobs[0].steps[] | .name + "=" + .conclusion] | join(", ")'))"
$K get pod "$runner_name" >/dev/null 2>&1 && ok "runner $runner_name is an in-cluster forge-runner pod" || fail "runner $runner_name is not a pod in the cluster"
arts=$(api "$BASE/api/v3/repos/$OWNER/$repo/actions/runs/$run_id/artifacts")
aid=$(echo "$arts" | jq -r '.artifacts[] | select(.name=="smoke-out") | .id')
[ -n "$aid" ] || fail "artifact smoke-out missing: $arts"
api -L -o "$W/art.zip" "$BASE/api/v3/repos/$OWNER/$repo/actions/artifacts/$aid/zip"
mkdir -p "$W/art"; unzip -q -o "$W/art.zip" -d "$W/art" || fail "artifact zip invalid"
[ "$(cat "$W/art/sha.txt")" = "$sha" ] || fail "artifact sha.txt != $sha"
( cd "$W/art" && tar tzf src.tgz | grep -qx README.md ) || fail "artifact src.tgz content"
ok "artifact smoke-out ($(wc -c < "$W/art.zip" | tr -d ' ') B zip): $(cd "$W/art" && ls | tr '\n' ' ')sha.txt matches"

# ------------------------------------------------------------------ 3. OCI registry
log "OCI: docker push/pull $HOSTPORT/$OWNER/smoke-img"
DOCKER_HOST_SOCK=$(docker context inspect --format '{{.Endpoints.docker.Host}}' 2>/dev/null || echo unix:///var/run/docker.sock)
mkdir -p "$W/img" "$W/dockercfg"
echo '{"auths":{}}' > "$W/dockercfg/config.json"
head -c 300000 /dev/urandom > "$W/img/payload.bin"
echo "smoke $STAMP" > "$W/img/hello.txt"
printf 'FROM scratch\nCOPY hello.txt payload.bin /\n' > "$W/img/Dockerfile"
img="$HOSTPORT/$OWNER/smoke-img:$STAMP"
docker build -q --provenance=false --sbom=false -t "$img" "$W/img" >/dev/null || fail "docker build"
id_before=$(docker image inspect "$img" --format '{{.Id}}')
dk() { DOCKER_CONFIG="$W/dockercfg" DOCKER_HOST="$DOCKER_HOST_SOCK" docker "$@"; }
echo "$TOKEN" | dk login "$HOSTPORT" -u "$OWNER" --password-stdin >/dev/null 2>&1 || fail "docker login $HOSTPORT"
dk push -q "$img" >/dev/null || fail "docker push"
ok "docker push $img"
tags=$(api "$BASE/v2/$OWNER/smoke-img/tags/list" | jq -r '.tags | join(",")')
echo ",$tags," | grep -q ",$STAMP," || fail "tags/list lacks $STAMP ($tags)"
docker rmi -f "$img" >/dev/null
dk pull -q "$img" >/dev/null || fail "docker pull"
id_after=$(docker image inspect "$img" --format '{{.Id}}')
[ "$id_before" = "$id_after" ] || fail "image id changed ($id_before -> $id_after)"
cid=$(docker create "$img" /x); docker cp "$cid:/hello.txt" "$W/hello.out" >/dev/null; docker rm "$cid" >/dev/null
cmp -s "$W/img/hello.txt" "$W/hello.out" || fail "pulled image content differs"
ok "docker pull: same image id ${id_after:7:12}, file content matches; tags/list=$tags"
dk logout "$HOSTPORT" >/dev/null 2>&1 || true
docker rmi -f "$img" >/dev/null

# ------------------------------------------------------------------ 4. npm registry
log "npm: publish + install @$OWNER/smoke-pkg"
NPMRC="$W/npmrc"
printf '@%s:registry=%s/npm/\n//%s/npm/:_authToken=%s\n' "$OWNER" "$BASE" "$HOSTPORT" "$TOKEN" > "$NPMRC"
export npm_config_userconfig="$NPMRC" npm_config_cache="$W/npm-cache" npm_config_update_notifier=false npm_config_fund=false npm_config_audit=false
ver="1.0.$STAMP"
mkdir -p "$W/pkg" "$W/app"
cat > "$W/pkg/package.json" <<EOF
{"name":"@$OWNER/smoke-pkg","version":"$ver","main":"index.js","license":"MIT","description":"forge k8s smoke"}
EOF
echo "module.exports = { stamp: '$STAMP' };" > "$W/pkg/index.js"
( cd "$W/pkg" && npm publish --registry "$BASE/npm/" >/dev/null 2>"$W/npm-publish.err" ) || { cat "$W/npm-publish.err" >&2; fail "npm publish"; }
ok "npm publish @$OWNER/smoke-pkg@$ver"
echo '{"name":"smoke-app","version":"1.0.0","private":true}' > "$W/app/package.json"
( cd "$W/app" && npm install "@$OWNER/smoke-pkg@$ver" >/dev/null 2>"$W/npm-install.err" ) || { cat "$W/npm-install.err" >&2; fail "npm install"; }
got=$(cd "$W/app" && node -e "console.log(require('@$OWNER/smoke-pkg').stamp)")
[ "$got" = "$STAMP" ] || fail "require() returned '$got'"
ok "npm install + require(): stamp matches"

log "smoke-full $FLAVOR: PASS ($(since))"

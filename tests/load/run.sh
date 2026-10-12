#!/usr/bin/env bash
# Runs the forge k6 suite and writes a markdown summary to
# tests/load/results/<cluster>-<date>.md (raw k6 JSON summaries next to it).
#   tests/load/run.sh <base_url> <token>
# Env: CLUSTER (label; default: current kube-context minus k3d-/kind- prefix, else "local"),
#      DURATION per script (default 60s), READ_VUS (20), GIT_VUS (10), WRITE_VUS (5),
#      WRITE_RATE (api-write ops/s, default 50),
#      SCRIPTS (default "api-read git-fetch api-write"), OWNER (default: token's login).
set -euo pipefail
BASE=${1:?usage: run.sh <base_url> <token>}; TOKEN=${2:?usage: run.sh <base_url> <token>}
BASE=${BASE%/}
HERE=$(cd "$(dirname "$0")" && pwd)
export GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_GLOBAL=/dev/null GIT_TERMINAL_PROMPT=0
DURATION=${DURATION:-60s}
SCRIPTS=${SCRIPTS:-"api-read git-fetch api-write"}
if [ -z "${CLUSTER:-}" ]; then
  ctx=$(kubectl config current-context 2>/dev/null || true)
  case "$ctx" in k3d-*) CLUSTER="k3d";; kind-*) CLUSTER="kind";; *) CLUSTER="local";; esac
fi
STAMP=$(date +%Y%m%d-%H%M%S)
OUT="$HERE/results"; mkdir -p "$OUT"
REPORT="$OUT/$CLUSTER-$STAMP.md"
RAW="$OUT/raw/$CLUSTER-$STAMP"; mkdir -p "$RAW"

api() { curl -sS -H "Authorization: token $TOKEN" -H 'Accept: application/vnd.github+json' "$@"; }
OWNER=${OWNER:-$(api "$BASE/api/v3/user" | jq -r .login)}
[ -n "$OWNER" ] && [ "$OWNER" != null ] || { echo "token rejected by $BASE" >&2; exit 1; }

# ---- fixtures: 3 repos with history (~120 commits, ~60 files, a few hundred KB packed)
echo "==> preparing fixture repos as $OWNER"
REPOS=()
tmp=$(mktemp -d); trap 'rm -rf "$tmp"' EXIT
for i in 1 2 3; do
  name="loadfix-$i"
  REPOS+=("$OWNER/$name")
  code=$(api -o /dev/null -w '%{http_code}' -X POST "$BASE/api/v3/user/repos" -d "{\"name\":\"$name\",\"description\":\"k6 fixture\"}")
  if [ "$code" = 201 ]; then
    src="$tmp/$name"; git init -q -b main "$src"
    for c in $(seq 1 120); do
      f=$((c % 60))
      { echo "commit $c of fixture $i"; head -c 2000 /dev/urandom | base64; } > "$src/file$f.txt"
      [ $c = 1 ] && printf '# %s\n\nk6 load fixture\n' "$name" > "$src/README.md"
      git -C "$src" add -A
      git -C "$src" -c user.name=k6 -c user.email=k6@forge.local commit -q -m "fixture commit $c"
    done
    git -C "$src" push -q "http://x-access-token:$TOKEN@${BASE#http://}/$OWNER/$name.git" main
    echo "   created + pushed $OWNER/$name"
  else
    echo "   $OWNER/$name exists (HTTP $code), reusing"
  fi
done
REPO_LIST=$(IFS=,; echo "${REPOS[*]}")

# (bash 3.2 compatible: no associative arrays)
vus_for() { case "$1" in api-read) echo "${READ_VUS:-20}";; git-fetch) echo "${GIT_VUS:-10}";; api-write) echo "${WRITE_VUS:-5}";; *) echo 5;; esac; }
for s in $SCRIPTS; do
  echo "==> k6 $s ($(vus_for "$s") VUs, $DURATION)"
  k6 run --quiet \
    -e BASE_URL="$BASE" -e TOKEN="$TOKEN" -e OWNER="$OWNER" -e REPOS="$REPO_LIST" \
    -e VUS="$(vus_for "$s")" -e DURATION="$DURATION" -e RUN_ID="$STAMP" -e WRITE_RATE="${WRITE_RATE:-50}" \
    -e SUMMARY_JSON="$RAW/$s.json" \
    "$HERE/$s.js" | tee "$RAW/$s.txt" || echo "   (k6 $s: thresholds failed)"
done

# ---- markdown summary
{
  echo "# forge load test — $CLUSTER — $STAMP"
  echo
  echo "- target: \`$BASE\` (cluster: $CLUSTER); duration per script: $DURATION"
  echo "- forge pods: $(kubectl -n forge get deploy forge -o jsonpath='{.status.readyReplicas}' 2>/dev/null || echo '?') ready"
  echo "- image \`forge:dev\` $(docker image inspect forge:dev --format '{{.Id}}' 2>/dev/null | cut -c1-19); rendered manifests (\`kubectl kustomize deploy/overlays/$CLUSTER\`) sha256 $(kubectl kustomize "$HERE/../../deploy/overlays/$CLUSTER" 2>/dev/null | shasum -a 256 | cut -c1-12)"
  echo "- host: $(uname -sm), k6 $(k6 version | head -1 | awk '{print $2}')"
  echo
  echo "| script | VUs / rate | requests | req/s | iterations | http fail % | p50 ms | p95 ms | p99 ms | checks % | thresholds |"
  echo "|---|---|---|---|---|---|---|---|---|---|---|"
  for s in $SCRIPTS; do
    j="$RAW/$s.json"; [ -f "$j" ] || continue
    jq -r --arg s "$s" --arg v "$([ "$s" = api-write ] && echo "${WRITE_RATE:-50} ops/s" || vus_for "$s")" '
      .metrics as $m
      | ([$m[] | select(.thresholds) | .thresholds[] | .ok] | if all then "PASS" else "FAIL" end) as $th
      | [$s, $v,
         ($m.http_reqs.values.count|tostring),
         ($m.http_reqs.values.rate*10|round/10|tostring),
         ($m.iterations.values.count|tostring),
         ($m.http_req_failed.values.rate*10000|round/100|tostring),
         ($m.http_req_duration.values.med*10|round/10|tostring),
         ($m.http_req_duration.values["p(95)"]*10|round/10|tostring),
         (($m.http_req_duration.values["p(99)"] // 0)*10|round/10|tostring),
         ($m.checks.values.rate*10000|round/100|tostring),
         $th] | "| " + join(" | ") + " |"' "$j"
  done
  echo
  echo "## Per-endpoint / per-step latency"
  echo
  echo "| script | metric | p95 ms | threshold |"
  echo "|---|---|---|---|"
  for s in $SCRIPTS; do
    j="$RAW/$s.json"; [ -f "$j" ] || continue
    jq -r --arg s "$s" '.metrics | to_entries[] | select(.value.thresholds)
      | .key as $k | .value as $v | $v.thresholds | to_entries[]
      | "| \($s) | `\($k)` | \(($v.values["p(95)"] // $v.values.rate // 0)*10|round/10) | `\(.key)` \(if .value.ok then "PASS" else "FAIL" end) |"' "$j"
  done
  echo
  echo "Raw k6 summaries: \`tests/load/results/raw/$CLUSTER-$STAMP/\`"
} > "$REPORT"
echo "==> wrote $REPORT"
cat "$REPORT"

#!/usr/bin/env bash
# Performance scenarios for one deployed app (forge or Forgejo), RUNS times each, interleaved
# (run 1 of every scenario, then run 2, ...) so slow drift on the host hits both runs equally.
#   bench.sh data/bench/<app>.env <label>          e.g. bench.sh data/bench/forge.env forge-1
# Env: RUNS (2), DURATION (60s), SCENARIOS (default: all below), SAT_VUS (64)
# Raw k6 output: tests/baseline/results/raw/<label>/<scenario>-run<N>.{json,txt}
#
# scenarios:
#   read-r100 read-r500 read-r1000   API read mix, open model at 100/500/1000 req/s
#   read-sat                         API read mix, closed loop with SAT_VUS VUs (saturation)
#   git-c1 git-c8 git-c32            git clone/fetch (tests/load/git-fetch.js), 1/8/32 VUs
#   write-r10 write-r25 write-r50    create issue + comment, 10/25/50 iterations/s (2 req each)
set -uo pipefail
# keep the host awake for the whole run (a laptop idle/maintenance sleep freezes the VM and k6)
[ -z "${BENCH_CAFFEINATED:-}" ] && command -v caffeinate >/dev/null && BENCH_CAFFEINATED=1 exec caffeinate -ims "$0" "$@"
ENVF=${1:?usage: bench.sh data/bench/<app>.env <label>}; LABEL=${2:?label}
# shellcheck disable=SC1090
source "$ENVF"
HERE=$(cd "$(dirname "$0")" && pwd); ROOT=$(cd "$HERE/../.." && pwd)
RUNS=${RUNS:-2}; DURATION=${DURATION:-60s}; SAT_VUS=${SAT_VUS:-64}
SCENARIOS=${SCENARIOS:-"read-r100 read-r500 read-r1000 read-sat git-c1 git-c8 git-c32 write-r10 write-r25 write-r50"}
RAW="$HERE/results/raw/$LABEL"; mkdir -p "$RAW"
REPOS="$OWNER/loadfix-1,$OWNER/loadfix-2,$OWNER/loadfix-3"
WREPOS="$OWNER/writefix-1,$OWNER/writefix-2,$OWNER/writefix-3"
K="kubectl --context $CTX -n $NS"

k6r() { # out_basename script extra -e args...
  local out=$1 script=$2; shift 2
  k6 run --quiet -e APP="$APP" -e BASE_URL="$BASE" -e TOKEN="$TOKEN" -e OWNER="$OWNER" \
    -e REPOS="$REPOS" -e WREPOS="$WREPOS" -e DURATION="$DURATION" -e SUMMARY_JSON="$out.json" \
    "$@" "$script" > "$out.txt" 2>&1 || true
}
run_one() { # scenario run
  local s=$1 r=$2 out="$RAW/$1-run$2"
  # resource snapshot ~2/3 into the run (pods of the app + its Postgres)
  # and the host load average (other work on the machine skews results; see docs/baseline.md)
  ( sleep 40; { uptime; $K top pods --no-headers; } > "$out.top" 2>/dev/null ) &
  case $s in
    read-sat) k6r "$out" "$HERE/k6/api-read.js" -e MODE=vus -e VUS="$SAT_VUS" ;;
    read-r*)  k6r "$out" "$HERE/k6/api-read.js" -e MODE=rate -e RATE="${s#read-r}" ;;
    git-c*)   k6r "$out" "$ROOT/tests/load/git-fetch.js" -e VUS="${s#git-c}" ;;
    write-r*) k6r "$out" "$HERE/k6/api-write.js" -e MODE=rate -e RATE="${s#write-r}" ;;
    *) echo "unknown scenario $s" >&2; return ;;
  esac
  wait
  jq -r --arg s "$s" --arg r "$r" '.metrics as $m | "   \($s) run \($r): \($m.http_reqs.values.rate|floor) req/s, p50 \($m.http_req_duration.values.med*10|round/10) p95 \($m.http_req_duration.values["p(95)"]*10|round/10) p99 \($m.http_req_duration.values["p(99)"]*10|round/10) ms, failed \($m.http_req_failed.values.rate*10000|round/100)%, dropped \($m.dropped_iterations.values.count // 0)"' "$out.json" 2>/dev/null || echo "   $s run $r: no summary"
  sleep 5
}

{
  echo "label=$LABEL app=$APP replicas=$REPLICAS base=$BASE runs=$RUNS duration=$DURATION started=$(date -u +%FT%TZ)"
  echo "host=$(uname -sm) k6=$(k6 version | head -1 | awk '{print $2}') docker_cpus=$(docker info --format '{{.NCPU}}') docker_mem=$(docker info --format '{{.MemTotal}}')"
  $K get pods -o wide
  $K get pods -o jsonpath='{range .items[*]}{.metadata.name} {.status.containerStatuses[0].imageID}{"\n"}{end}'
} > "$RAW/meta.txt"
echo "==> $LABEL: warm-up (not recorded)"
DURATION=20s k6r "$RAW/warmup-read" "$HERE/k6/api-read.js" -e MODE=rate -e RATE=200
DURATION=15s k6r "$RAW/warmup-git" "$ROOT/tests/load/git-fetch.js" -e VUS=4
for r in $(seq 1 "$RUNS"); do
  for s in $SCENARIOS; do run_one "$s" "$r"; done
done
echo "finished=$(date -u +%FT%TZ)" >> "$RAW/meta.txt"
echo "==> $LABEL done: $RAW"

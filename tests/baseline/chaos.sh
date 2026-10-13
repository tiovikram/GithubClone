#!/usr/bin/env bash
# Pod-kill chaos parity: phases A1 (graceful `kubectl delete pod`) and A2 (`--grace-period=0
# --force`) of tests/chaos/pod-kill.sh, applied the same way to forge or Forgejo:
# $KILLS kills, one every $INTERVAL s, of a random app pod while k6 runs the baseline read mix
# (10 VUs) + git clone/fetch (4 VUs) with a 10 s request timeout, and
# tests/chaos/git-integrity-loop.sh pushes/clones/fscks continuously. A 0.5 s availability probe
# (GET /users/<owner>) measures the outage window per kill.
#   chaos.sh data/bench/<app>.env <label>
# Results: tests/baseline/results/chaos-<label>.md, raw in results/raw/chaos-<label>/
set -uo pipefail
[ -z "${BENCH_CAFFEINATED:-}" ] && command -v caffeinate >/dev/null && BENCH_CAFFEINATED=1 exec caffeinate -ims "$0" "$@"
ENVF=${1:?usage: chaos.sh data/bench/<app>.env <label>}; LABEL=${2:?label}
# shellcheck disable=SC1090
source "$ENVF"
HERE=$(cd "$(dirname "$0")" && pwd); ROOT=$(cd "$HERE/../.." && pwd)
KILLS=${KILLS:-6}; INTERVAL=${INTERVAL:-15}; PHASES=${PHASES:-"A1 A2"}
case $APP in forgejo) API=$BASE/api/v1;; *) API=$BASE/api/v3;; esac
K="kubectl --context $CTX -n $NS"
SEL="app.kubernetes.io/name=$APP"
export GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_GLOBAL=/dev/null GIT_TERMINAL_PROMPT=0
STAMP=$(date +%Y%m%d-%H%M%S)
RAW="$HERE/results/raw/chaos-$LABEL"; mkdir -p "$RAW"
REPORT="$HERE/results/chaos-$LABEL.md"
W=$(mktemp -d); trap 'touch "$W"/*/stop 2>/dev/null; rm -rf "$W"' EXIT
REPOS="$OWNER/loadfix-1,$OWNER/loadfix-2,$OWNER/loadfix-3"
URLP="http://x-access-token:$TOKEN@${BASE#http://}/$OWNER"
now() { date +%s; }
md() { echo "$*" >> "$REPORT"; }
api() { curl -sS -m 10 -H "Authorization: token $TOKEN" -H 'Content-Type: application/json' "$@"; }
ready() { $K get deploy "$APP" -o jsonpath='{.status.readyReplicas}' 2>/dev/null; }
wait_ready() { local t0; t0=$(now); for _ in $(seq 1 300); do [ "$(ready)" -ge "$REPLICAS" ] 2>/dev/null && break; sleep 1; done; echo $(( $(now) - t0 )); }

{
  echo "# Chaos parity — $LABEL ($APP, $REPLICAS replica(s)) — $STAMP"
  echo
  echo "- target \`$BASE\`; $KILLS kills every ${INTERVAL}s per phase; k6 read mix 10 VUs + git-fetch 4 VUs, 10 s timeout"
  echo "- image: \`$($K get pods -l "$SEL" -o jsonpath='{.items[0].status.containerStatuses[0].imageID}')\`"
  echo
} > "$REPORT"

run_phase() { # name flags
  local name=$1 flags=$2 dur=$(( KILLS * INTERVAL + 20 )) repo="chaos-$1-$STAMP" lw="$W/$1"
  echo "==> $LABEL phase $name: ${dur}s, $KILLS kills ($flags)"
  api -o /dev/null -X POST "$API/user/repos" -d "{\"name\":\"$repo\"}"
  bash "$ROOT/tests/chaos/git-integrity-loop.sh" "$BASE" "$TOKEN" "$OWNER" "$repo" "$lw" & local loop=$!
  k6 run --quiet -e APP="$APP" -e BASE_URL="$BASE" -e TOKEN="$TOKEN" -e OWNER="$OWNER" -e REPOS="$REPOS" \
    -e MODE=vus -e VUS=10 -e DURATION="${dur}s" -e HTTP_TIMEOUT=10s -e SUMMARY_JSON="$RAW/$name-api-read.json" \
    "$HERE/k6/api-read.js" > "$RAW/$name-api-read.txt" 2>&1 & local k1=$!
  k6 run --quiet -e BASE_URL="$BASE" -e TOKEN="$TOKEN" -e OWNER="$OWNER" -e REPOS="$REPOS" \
    -e VUS=4 -e DURATION="${dur}s" -e HTTP_TIMEOUT=10s -e SUMMARY_JSON="$RAW/$name-git-fetch.json" \
    "$ROOT/tests/load/git-fetch.js" > "$RAW/$name-git-fetch.txt" 2>&1 & local k2=$!
  # availability probe: one line per 0.5 s, "<epoch_ms> <http code>"
  ( while [ ! -e "$lw/stop" ]; do
      c=$(curl -s -o /dev/null -m 2 -w '%{http_code}' -H "Authorization: token $TOKEN" "$API/users/$OWNER")
      echo "$(perl -MTime::HiRes=time -e 'printf "%d", time*1000') $c"; sleep 0.5
    done > "$RAW/$name-probe.txt" ) & local probe=$!
  sleep 10
  md "## Phase $name — \`kubectl delete pod $flags\` every ${INTERVAL}s"
  md
  md "| kill | pod | all $REPLICAS ready again after |"
  md "|---|---|---|"
  for k in $(seq 1 "$KILLS"); do
    local pods pod t0 rec
    pods=($($K get pods -l "$SEL" --field-selector=status.phase=Running -o name))
    pod=${pods[$((RANDOM % ${#pods[@]}))]#pod/}
    t0=$(now)
    # shellcheck disable=SC2086
    $K delete pod "$pod" --wait=false $flags >/dev/null 2>&1
    echo "$(( t0 * 1000 )) KILL $pod" >> "$RAW/$name-kills.txt"
    sleep 2; rec=$(wait_ready)
    md "| $k | $pod | $(( rec + 2 ))s |"
    echo "   kill $k: $pod -> ready after $(( rec + 2 ))s"
    local rest=$(( INTERVAL - ($(now) - t0) )); [ $rest -gt 0 ] && sleep $rest
  done
  wait $k1 $k2
  touch "$lw/stop"; wait $loop $probe
  # shellcheck disable=SC1091
  source "$lw/stats"
  md
  md "| k6 script | requests | failed | error rate | p50 ms | p95 ms | p99 ms |"
  md "|---|---|---|---|---|---|---|"
  local total=0 failed=0
  for s in api-read git-fetch; do
    local j="$RAW/$name-$s.json"
    [ -f "$j" ] || { md "| $s | no summary | | | | | |"; continue; }
    local n f
    n=$(jq '.metrics.http_reqs.values.count' "$j"); f=$(jq '.metrics.http_req_failed.values.passes' "$j")
    total=$((total + n)); failed=$((failed + f))
    md "$(jq -r --arg s "$s" '.metrics as $m | "| \($s) | \($m.http_reqs.values.count) | \($m.http_req_failed.values.passes) | \($m.http_req_failed.values.rate*100000|round/1000)% | \($m.http_req_duration.values.med*10|round/10) | \($m.http_req_duration.values["p(95)"]*10|round/10) | \($m.http_req_duration.values["p(99)"]*10|round/10) |"' "$j")"
  done
  # outage: longest run of consecutive non-200 probes, and total non-200 probe time
  local outage
  outage=$(awk '{ if ($2 != 200) { if (!s) s=$1; e=$1; bad++ } else if (s) { d=e-s+500; if (d>m) m=d; s=0 } }
                END { if (s) { d=e-s+500; if (d>m) m=d }; printf "longest %.1fs, total %.1fs of %d probes failed", m/1000, bad*0.5, bad }' "$RAW/$name-probe.txt")
  md
  md "- overall HTTP error rate: **$(echo "scale=3; 100 * $failed / ($total + 0.000001)" | bc -l)%** ($failed / $total)"
  md "- availability probe (GET /users/$OWNER every 0.5 s): $outage"
  md "- git integrity loop: $iter iterations, pushes ok/failed **$push_ok/$push_fail**, clones ok/failed **$clone_ok/$clone_fail**, fsck failures **$fsck_fail**, acked commits missing **$missing**"
  md
  sed -E "s/$TOKEN/<token>/g" "$lw/log" > "$RAW/$name-git-loop.log" 2>/dev/null
}

[[ " $PHASES " == *" A1 "* ]] && run_phase A1-graceful ""
[[ " $PHASES " == *" A2 "* ]] && run_phase A2-force "--grace-period=0 --force"
md "Container restarts at the end: $($K get pods -l "$SEL" -o jsonpath='{range .items[*]}{.status.containerStatuses[0].restartCount}{" "}{end}')"
echo "==> wrote $REPORT"; cat "$REPORT"

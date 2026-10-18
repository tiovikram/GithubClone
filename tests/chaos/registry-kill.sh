#!/usr/bin/env bash
# Chaos: kill forge pods during a chunked OCI blob upload.
#   tests/chaos/registry-kill.sh k3d|kind        (reads data/k8s/<flavor>.env written by up.sh)
#
# tests/chaos/oci_chunked_upload.py uploads a $SIZE_MB MB random blob in $CHUNK_MB MB PATCH
# chunks paced at $RATE MB/s (so a kill lands mid-chunk) through the NodePort, resuming from the
# session's Range after any error. While it runs, forge pods are killed:
#   crash-all   `kill -9` of `forge serve` in every forge pod at once (all containers restart)
#   force-x2    `kubectl delete pod --grace-period=0 --force` of a random pod, twice
#   graceful-x2 `kubectl delete pod` of a random pod, twice
# Pass: the upload completes (201 with the right Docker-Content-Digest), the blob downloads 3
# times with the right sha256 and size, and Postgres' package_blobs row has the right size.
# Results: tests/chaos/results/registry-<flavor>-<date>.md
set -uo pipefail
FLAVOR=${1:?usage: registry-kill.sh k3d|kind}
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
# shellcheck disable=SC1090
source "$ROOT/data/k8s/$FLAVOR.env"
BASE=$FORGE_URL; TOKEN=$FORGE_TOKEN
K="kubectl --context $KUBE_CONTEXT -n forge"
CASES=${CASES:-"crash-all force-x2 graceful-x2 crash-all"}
SIZE_MB=${SIZE_MB:-96}; CHUNK_MB=${CHUNK_MB:-8}; RATE=${RATE:-8}
STAMP=$(date +%Y%m%d-%H%M%S)
OUT="$ROOT/tests/chaos/results"; RAW="$OUT/raw/registry-$FLAVOR-$STAMP"; mkdir -p "$RAW"
REPORT="$OUT/registry-$FLAVOR-$STAMP.md"
OWNER=$(curl -sS -H "Authorization: token $TOKEN" "$BASE/api/v3/user" | jq -r .login)
VERDICT=PASS
md() { echo "$*" >> "$REPORT"; }
fail() { VERDICT=FAIL; md "- **FAIL**: $*"; echo "FAIL: $*" >&2; }
psql_q() { $K exec postgres-0 -- psql -U forge -d forge -tAc "$1" 2>/dev/null; }
forge_pods() { $K get pods -l app.kubernetes.io/name=forge --field-selector=status.phase=Running -o name | sed 's#pod/##'; }
SIGKILL_FORGE='for d in /proc/[0-9]*; do [ "$(tr "\0" " " < $d/cmdline 2>/dev/null)" = "forge serve " ] && kill -9 ${d#/proc/}; done'
wait_chunk() { # $1 events file, $2 chunk number; waits until the client has committed chunk $2
  for _ in $(seq 1 600); do grep -q "chunk $2 " "$1" 2>/dev/null && return 0; grep -q finished "$1" 2>/dev/null && return 1; sleep 0.1; done; return 1
}
wait_ready() { for _ in $(seq 1 180); do [ "$($K get deploy forge -o jsonpath='{.status.readyReplicas}')" -ge 3 ] 2>/dev/null && return; sleep 1; done; }

{
  echo "# forge registry chaos (chunked OCI blob upload) — $FLAVOR — $STAMP"
  echo
  echo "- target \`$BASE/v2/\`; blob ${SIZE_MB} MB random, ${CHUNK_MB} MB PATCH chunks paced at ${RATE} MB/s; client resumes from the session Range after errors"
  echo
  echo "| run | kill | kills landed after chunk | upload result | client errors / resumes / restarts | resumed from (MB) | elapsed | downloads ok (sha256+size) | package_blobs size ok |"
  echo "|---|---|---|---|---|---|---|---|---|"
} > "$REPORT"

n=0
for case in $CASES; do
  n=$((n+1))
  ev="$RAW/run$n.events"; : > "$ev"
  name="$OWNER/chaos-blob-$n"
  python3 -I "$ROOT/tests/chaos/oci_chunked_upload.py" "$BASE" "$TOKEN" "$name" "$SIZE_MB" "$CHUNK_MB" "$RATE" "$ev" > "$RAW/run$n.json" 2> "$RAW/run$n.err" &
  cpid=$!
  landed=""
  case $case in
    crash-all)
      k=$(( 2 + RANDOM % 5 ))
      if wait_chunk "$ev" $k; then
        sleep 0.$(( RANDOM % 9 ))
        kp=()
        for p in $(forge_pods); do $K exec "$p" -c forge -- bash -c "$SIGKILL_FORGE" >/dev/null 2>&1 & kp+=($!); done
        [ ${#kp[@]} -gt 0 ] && wait "${kp[@]}"
        landed="$k"
      fi ;;
    force-x2|graceful-x2)
      flags=""; [ $case = force-x2 ] && flags="--grace-period=0 --force"
      for k in 2 7; do
        if wait_chunk "$ev" $k; then
          sleep 0.$(( RANDOM % 9 ))
          pods=($(forge_pods)); p=${pods[$((RANDOM % ${#pods[@]}))]}
          # shellcheck disable=SC2086
          $K delete pod "$p" --wait=false $flags >/dev/null 2>&1
          landed="${landed:+$landed, }$k"
        fi
      done ;;
  esac
  wait $cpid
  res=$(cat "$RAW/run$n.json")
  echo "   run $n ($case, kills after chunk ${landed:-none}): $res"
  ok=$(echo "$res" | jq -r '.ok // false'); digest=$(echo "$res" | jq -r '.digest // empty'); size=$(echo "$res" | jq -r '.size // 0')
  dbsize=$(psql_q "SELECT size FROM package_blobs WHERE digest = '$digest'")
  dbok=$([ "$dbsize" = "$size" ] && echo yes || echo "no ($dbsize)")
  [ "$ok" = true ] || fail "run $n ($case): upload/verify failed: $(echo "$res" | jq -c '{error, errors}')"
  [ "$dbok" = yes ] || fail "run $n: package_blobs size $dbsize != $size"
  [ -n "$landed" ] || fail "run $n: no kill landed during the upload"
  md "| $n | $case | ${landed:-none} | $([ "$ok" = true ] && echo "201, digest ok" || echo FAILED) | $(echo "$res" | jq -r '"\(.retries) / \(.resumed_from|length) / \(.restarts)"') | $(echo "$res" | jq -r '[.resumed_from[] | . / 1048576 | floor | tostring] | join(", ")') | $(echo "$res" | jq -r .elapsed_s)s | $(echo "$res" | jq -r '[.verify[] | if . then "ok" else "BAD" end] | join(" ")') | $dbok |"
  wait_ready
done
md
md "- open upload sessions left in Postgres (from restarted uploads; \`forge gc\` purges stale ones): $(psql_q 'SELECT count(*) FROM package_uploads')"
md "- forge container restarts at the end: $($K get pods -l app.kubernetes.io/name=forge -o jsonpath='{range .items[*]}{.status.containerStatuses[0].restartCount}{" "}{end}')"
md "- client error samples: \`tests/chaos/results/raw/registry-$FLAVOR-$STAMP/run*.json\`"
md
md "## Verdict: **$VERDICT**"
echo "==> $VERDICT — wrote $REPORT"
cat "$REPORT"
[ "$VERDICT" = PASS ]

#!/usr/bin/env bash
# Pod-kill chaos test for forge on a local cluster created by scripts/k8s/up.sh.
#   tests/chaos/pod-kill.sh k3d|kind            (reads data/k8s/<flavor>.env written by up.sh)
#
# Phases
#   A1  graceful kills: k6 read + git-fetch load and a push/clone integrity loop run while a random
#       forge pod is `kubectl delete`d every $INTERVAL s ($KILLS times). Error rate must stay
#       < $MAX_ERR_GRACEFUL.
#   A2  forced kills: same with `--grace-period=0 --force` (endpoint removed at once, kubelet
#       kills with its minimum grace). Error rate must stay < $MAX_ERR_FORCE.
#   B   mid-push kills: a large push is started, the pod running its receive-pack is found and
#       crashed (`kill -9` of forge, odd runs) or force-deleted (even runs). The ref must be
#       atomic (old or new, never torn), the repo must accept a retry push (advisory lock
#       released) and fresh clones must pass `git fsck --strict`.
#   C   dependency kills: minio-0 and postgres-0 are deleted once each; measures time until
#       forge serves pushes again and re-verifies every acknowledged commit.
# In every phase: all pushes git reported as successful must be present in later clones.
# Results: tests/chaos/results/<flavor>-<date>.md
set -uo pipefail
FLAVOR=${1:?usage: pod-kill.sh k3d|kind}
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
ENVF="$ROOT/data/k8s/$FLAVOR.env"
[ -f "$ENVF" ] || { echo "missing $ENVF (run scripts/k8s/up.sh $FLAVOR)" >&2; exit 1; }
# shellcheck disable=SC1090
source "$ENVF"
BASE=$FORGE_URL; TOKEN=$FORGE_TOKEN
K="kubectl --context $KUBE_CONTEXT -n forge"
KILLS=${KILLS:-6}; INTERVAL=${INTERVAL:-15}
MAX_ERR_GRACEFUL=${MAX_ERR_GRACEFUL:-0.005}; MAX_ERR_FORCE=${MAX_ERR_FORCE:-0.05}
MIDPUSH_RUNS=${MIDPUSH_RUNS:-4}
# Subsets: PHASES selects the load phases ("A1 A2", "none"); MIDPUSH_RUNS=0 skips B;
# DEP_TARGETS="" skips C.
PHASES=${PHASES:-"A1 A2"}; DEP_TARGETS=${DEP_TARGETS-"minio-0 postgres-0"}
export GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_GLOBAL=/dev/null GIT_TERMINAL_PROMPT=0
export GIT_HTTP_LOW_SPEED_LIMIT=1000 GIT_HTTP_LOW_SPEED_TIME=30
STAMP=$(date +%Y%m%d-%H%M%S)
OUT="$ROOT/tests/chaos/results"; mkdir -p "$OUT/raw/$FLAVOR-$STAMP"
RAW="$OUT/raw/$FLAVOR-$STAMP"
REPORT="$OUT/$FLAVOR-$STAMP.md"
W=$(mktemp -d); trap 'touch "$W"/*/stop 2>/dev/null; rm -rf "$W"' EXIT
api() { curl -sS -m 10 -H "Authorization: token $TOKEN" -H 'Accept: application/vnd.github+json' "$@"; }
OWNER=$(api "$BASE/api/v3/user" | jq -r .login)
URLP="http://x-access-token:$TOKEN@${BASE#http://}/$OWNER"
VERDICT=PASS
md() { echo "$*" >> "$REPORT"; }
fail() { VERDICT=FAIL; md "- **FAIL**: $*"; echo "FAIL: $*" >&2; }
now() { date +%s; }
# tmo SECS cmd...: run cmd in its own process group, SIGKILL the whole group after SECS (exit
# 124). macOS has no coreutils `timeout`; killing only the parent git would leave
# git-remote-http children blocked forever.
tmo() {
  perl -e '$t=shift; $p=fork; if(!$p){setpgrp(0,0); exec @ARGV; exit 127}
           $SIG{ALRM}=sub{kill "KILL", -$p; exit 124}; alarm $t; waitpid($p,0); exit($?>>8)' "$@"
}
mkrepo() { api -o /dev/null -w '%{http_code}' -X POST "$BASE/api/v3/user/repos" -d "{\"name\":\"$1\"}"; }
ready_forge() { $K get deploy forge -o jsonpath='{.status.readyReplicas}' 2>/dev/null; }
wait_forge_ready() { # waits until 3 ready replicas; echoes seconds waited
  local t0; t0=$(now)
  for _ in $(seq 1 180); do [ "$(ready_forge)" -ge 3 ] 2>/dev/null && break; sleep 1; done
  echo $(( $(now) - t0 ))
}

{
  echo "# forge chaos test — $FLAVOR — $STAMP"
  echo
  echo "- target \`$BASE\`, context \`$KUBE_CONTEXT\`; $KILLS kills every ${INTERVAL}s per phase"
  echo "- forge image: \`$($K get deploy forge -o jsonpath='{.spec.template.spec.containers[0].image}')\` (running imageID \`$($K get pods -l app.kubernetes.io/name=forge -o jsonpath='{.items[0].status.containerStatuses[0].imageID}')\`), replicas $(ready_forge)"
  echo
} > "$REPORT"

# Fixture repos for the k6 load (created by tests/load/run.sh; create minimal ones if absent).
REPOS=""
for i in 1 2 3; do
  if [ "$(mkrepo "loadfix-$i")" = 201 ]; then
    t="$W/fx$i"; git init -q -b main "$t"; echo "fixture $i" > "$t/README.md"
    git -C "$t" add -A && git -C "$t" -c user.name=c -c user.email=c@x commit -qm init && git -C "$t" push -q "$URLP/loadfix-$i.git" main
  fi
  REPOS="$REPOS${REPOS:+,}$OWNER/loadfix-$i"
done

# ---------------------------------------------------------------------------- phase A
run_phase_a() { # $1 = name, $2 = extra kubectl delete flags, $3 = max error rate
  local name=$1 flags=$2 maxerr=$3
  local dur=$(( KILLS * INTERVAL + 20 ))
  local repo="chaos-$name-$STAMP" lw="$W/$name"
  echo "==> phase $name: ${dur}s load, $KILLS kills ($flags)"
  [ "$(mkrepo "$repo")" = 201 ] || fail "could not create $repo"
  bash "$ROOT/tests/chaos/git-integrity-loop.sh" "$BASE" "$TOKEN" "$OWNER" "$repo" "$lw" &
  local loop_pid=$!
  for s in api-read git-fetch; do
    k6 run --quiet -e BASE_URL="$BASE" -e TOKEN="$TOKEN" -e OWNER="$OWNER" -e REPOS="$REPOS" \
      -e VUS=$([ $s = api-read ] && echo 10 || echo 4) -e DURATION="${dur}s" -e HTTP_TIMEOUT=10s \
      -e SUMMARY_JSON="$RAW/$name-$s.json" "$ROOT/tests/load/$s.js" > "$RAW/$name-$s.txt" 2>&1 &
  done
  sleep 10
  md "## Phase $name — \`kubectl delete pod $flags\` every ${INTERVAL}s"
  md
  md "| kill | pod | node | ready again after |"
  md "|---|---|---|---|"
  for k in $(seq 1 "$KILLS"); do
    local pods pod node t0 rec
    pods=($($K get pods -l app.kubernetes.io/name=forge --field-selector=status.phase=Running -o name))
    pod=${pods[$((RANDOM % ${#pods[@]}))]#pod/}
    node=$($K get pod "$pod" -o jsonpath='{.spec.nodeName}')
    t0=$(now)
    # shellcheck disable=SC2086
    $K delete pod "$pod" --wait=false $flags >/dev/null 2>&1
    sleep 2
    rec=$(wait_forge_ready)
    md "| $k | $pod | $node | $(( rec + 2 ))s |"
    echo "   kill $k: $pod ($node) -> 3/3 ready after $(( rec + 2 ))s"
    local rest=$(( INTERVAL - ($(now) - t0) )); [ $rest -gt 0 ] && sleep $rest
  done
  wait $(jobs -p | grep -v "^$loop_pid$") 2>/dev/null   # k6 runs
  touch "$lw/stop"; wait $loop_pid
  # Final integrity verification on a fresh clone after the dust settles.
  rm -rf "$lw/final"
  local final_ok=1
  git clone -q "$URLP/$repo.git" "$lw/final" || final_ok=0
  local missing_final=0 acked; acked=$(wc -l < "$lw/acked" | tr -d ' ')
  if [ $final_ok = 1 ]; then
    git -C "$lw/final" fsck --strict --no-progress >/dev/null 2>&1 || { fail "$name: final fsck failed"; }
    while read -r sha; do git -C "$lw/final" merge-base --is-ancestor "$sha" origin/main 2>/dev/null || missing_final=$((missing_final+1)); done < "$lw/acked"
  else fail "$name: final clone failed"; fi
  # shellcheck disable=SC1091
  source "$lw/stats"
  md
  md "| k6 script | requests | failed | error rate | p95 ms | p99 ms |"
  md "|---|---|---|---|---|---|"
  local total=0 failed=0
  for s in api-read git-fetch; do
    local j="$RAW/$name-$s.json"
    [ -f "$j" ] || { fail "$name: k6 $s produced no summary"; continue; }
    local n r p95 p99 f
    n=$(jq '.metrics.http_reqs.values.count' "$j"); r=$(jq '.metrics.http_req_failed.values.rate' "$j")
    p95=$(jq '.metrics.http_req_duration.values["p(95)"]*10|round/10' "$j"); p99=$(jq '.metrics.http_req_duration.values["p(99)"]*10|round/10' "$j")
    f=$(jq '.metrics.http_req_failed.values.passes' "$j")
    total=$((total + n)); failed=$((failed + f))
    md "| $s | $n | $f | $(echo "$r*100" | bc -l | xargs printf '%.3f')% | $p95 | $p99 |"
  done
  local rate; rate=$(echo "$failed / ($total + 0.000001)" | bc -l)
  md
  md "- overall HTTP error rate: **$(echo "$rate*100" | bc -l | xargs printf '%.3f')%** ($failed / $total), threshold < $(echo "$maxerr*100" | bc -l | xargs printf '%.1f')% → $(echo "$rate < $maxerr" | bc -l | grep -q 1 && echo PASS || echo FAIL)"
  echo "$rate < $maxerr" | bc -l | grep -q 1 || fail "$name: error rate $rate >= $maxerr"
  md "- git integrity loop: $iter iterations, pushes ok/failed **$push_ok/$push_fail**, clones ok/failed **$clone_ok/$clone_fail**, fsck failures **$fsck_fail**, acked commits missing in later clones **$missing**"
  md "- final fresh clone: fsck --strict clean, $((acked - missing_final))/$acked acknowledged commits present"
  [ "$fsck_fail" = 0 ] || fail "$name: $fsck_fail fsck failures"
  [ "$missing" = 0 ] && [ "$missing_final" = 0 ] || fail "$name: acknowledged commits missing ($missing during, $missing_final final)"
  [ "$push_ok" -gt 0 ] || fail "$name: no successful pushes"
  md
  # (redact the token: git error messages can echo the remote URL)
  sed -E "s/$TOKEN/<token>/g" "$lw/log" > "$RAW/$name-git-loop.log" 2>/dev/null
}

[[ " $PHASES " == *" A1 "* ]] && run_phase_a A1-graceful "" "$MAX_ERR_GRACEFUL"
[[ " $PHASES " == *" A2 "* ]] && run_phase_a A2-force "--grace-period=0 --force" "$MAX_ERR_FORCE"

# ---------------------------------------------------------------------------- phase B
# Odd runs: SIGKILL the `forge serve` process inside the container (a real crash: no preStop,
# no drain; the container restarts and keeps its emptyDir cache with whatever partial state the
# crash left). Even runs: `kubectl delete pod --grace-period=0 --force` (the pod is replaced).
echo "==> phase B: kill the forge pod handling a push, $MIDPUSH_RUNS times"
md "## Phase B — forge killed mid-push"
md
md "Odd runs: \`kill -9\` of the \`forge serve\` process handling the push (crash; container restarts with its"
md "emptyDir cache intact). Even runs: \`kubectl delete pod --grace-period=0 --force\` of that pod."
md
md "| run | push size | pod | kill | when | push result | ref after kill | retry push ok after | fresh clone fsck | content match |"
md "|---|---|---|---|---|---|---|---|---|---|"
find_pusher() { # echoes the forge pod currently running receive-pack/index-pack/repack, if any
  local p procs
  for p in $($K get pods -l app.kubernetes.io/name=forge --field-selector=status.phase=Running -o name); do
    # (no `| grep -q` directly: with pipefail, kubectl's SIGPIPE would make the match "fail")
    procs=$($K exec "${p#pod/}" -c forge -- bash -c 'for f in /proc/[0-9]*/cmdline; do tr "\0" " " < "$f" 2>/dev/null; echo; done' 2>/dev/null)
    if echo "$procs" | grep -E 'receive-pack|index-pack|repack' >/dev/null; then
      echo "${p#pod/}"; return 0
    fi
  done
  return 1
}
SIGKILL_FORGE='for d in /proc/[0-9]*; do [ "$(tr "\0" " " < $d/cmdline 2>/dev/null)" = "forge serve " ] && kill -9 ${d#/proc/}; done'
for run in $(seq 1 "$MIDPUSH_RUNS"); do
  repo="chaos-midpush-$run-$STAMP"; bw="$W/b$run"; mkdir -p "$bw"
  [ "$(mkrepo "$repo")" = 201 ] || fail "B$run: could not create repo"
  git init -q -b main "$bw/src"
  echo base > "$bw/src/README.md"; git -C "$bw/src" add -A; git -C "$bw/src" -c user.name=c -c user.email=c@x commit -qm base
  tmo 120 git -C "$bw/src" push -q "$URLP/$repo.git" main || fail "B$run: base push failed"
  old=$(git -C "$bw/src" rev-parse HEAD)
  # ~160 MB of incompressible data in 40 files: index-pack + S3 upload take a few seconds.
  for f in $(seq 1 40); do head -c 4000000 /dev/urandom > "$bw/src/big-$f.bin"; done
  git -C "$bw/src" add -A; git -C "$bw/src" -c user.name=c -c user.email=c@x commit -qm big
  new=$(git -C "$bw/src" rev-parse HEAD)
  # Watchdog: when the server dies mid-upload, stock git can hang forever (seen: socket in
  # CLOSE_WAIT, git-remote-http blocked on its pipe from send-pack). 120s, then counted as failed.
  tmo 120 git -C "$bw/src" push -q "$URLP/$repo.git" main > "$bw/push.out" 2>&1 &
  push_pid=$!
  victim=""; when="-"; how="-"
  for _ in $(seq 1 100); do
    victim=$(find_pusher) && break
    kill -0 $push_pid 2>/dev/null || break
    sleep 0.1
  done
  if [ -n "$victim" ]; then
    # Random extra delay so kills land in receive-pack/index-pack, repack, S3 upload or commit.
    d=0.$(( RANDOM % 7 )); sleep "$d"
    when="receive started +${d}s"
    if [ $((run % 2)) = 1 ]; then
      how="kill -9 forge"
      $K exec "$victim" -c forge -- bash -c "$SIGKILL_FORGE" >/dev/null 2>&1
    else
      how="force delete pod"
      $K delete pod "$victim" --grace-period=0 --force >/dev/null 2>&1
    fi
  else
    when="not caught (push finished first)"
  fi
  wait $push_pid; prc=$?
  case $prc in 0) presult="acked";; 124) presult="failed (client hung, watchdog)";; *) presult="failed (rc $prc)";; esac
  ref=$(tmo 60 git ls-remote "$URLP/$repo.git" refs/heads/main 2>/dev/null | cut -f1)
  case "$ref" in "$old") refst="old (atomic)";; "$new") refst="new (atomic)";; *) refst="TORN: $ref"; fail "B$run: ref is neither old nor new";; esac
  [ $prc = 0 ] && [ "$ref" != "$new" ] && fail "B$run: push acked but ref not updated"
  t0=$(now); retry="never"
  for _ in $(seq 1 30); do
    if tmo 120 git -C "$bw/src" push -q "$URLP/$repo.git" main 2>>"$bw/push.out"; then retry="$(( $(now) - t0 ))s"; break; fi
    sleep 2
  done
  [ "$retry" = never ] && fail "B$run: retry push never succeeded (lock not released?)"
  rm -rf "$bw/clone"
  fsck="FAIL"; match="no"
  if tmo 120 git clone -q "$URLP/$repo.git" "$bw/clone"; then
    git -C "$bw/clone" fsck --strict --no-progress >/dev/null 2>&1 && fsck="clean"
    [ "$(git -C "$bw/clone" rev-parse HEAD)" = "$new" ] && diff -rq -x .git "$bw/src" "$bw/clone" >/dev/null && match="yes"
  fi
  [ "$fsck" = clean ] || fail "B$run: fsck failed"
  [ "$match" = yes ] || fail "B$run: clone content differs"
  md "| $run | 160 MB | ${victim:--} | $how | $when | $presult | $refst | $retry | $fsck | $match |"
  echo "   B$run: victim=${victim:--} ($how, $when) push=$presult ref=$refst retry=$retry fsck=$fsck match=$match"
  wait_forge_ready >/dev/null
done
md
md

# ---------------------------------------------------------------------------- phase C
echo "==> phase C: kill minio-0 and postgres-0"
md "## Phase C — dependency pods deleted (graceful)"
md
md "| pod | back Ready after | first successful push after | acked commits verified |"
md "|---|---|---|---|"
repo="chaos-deps-$STAMP"; cw="$W/c"; mkdir -p "$cw"
[ "$(mkrepo "$repo")" = 201 ] || fail "C: could not create repo"
git init -q -b main "$cw/src"; : > "$cw/acked"
cpush() { # commit + push; record acked sha
  echo "$(date +%s) $RANDOM" >> "$cw/src/log.txt"; git -C "$cw/src" add -A
  git -C "$cw/src" -c user.name=c -c user.email=c@x commit -qm "c $(date +%s)"
  git -C "$cw/src" push -q "$URLP/$repo.git" main 2>/dev/null && git -C "$cw/src" rev-parse HEAD >> "$cw/acked"
}
for i in 1 2 3; do cpush; done
for target in $DEP_TARGETS; do
  sts=${target%-0}
  t0=$(now)
  $K delete pod "$target" --wait=false >/dev/null 2>&1
  sleep 3
  $K wait --for=condition=Ready "pod/$target" --timeout=300s >/dev/null 2>&1
  t_ready=$(( $(now) - t0 ))
  t_push="never"
  for _ in $(seq 1 90); do if cpush; then t_push="$(( $(now) - t0 ))s"; break; fi; sleep 2; done
  [ "$t_push" = never ] && fail "C: no successful push after $target restart"
  wait_forge_ready >/dev/null
  rm -rf "$cw/clone"; ok=0; total=$(wc -l < "$cw/acked" | tr -d ' ')
  if git clone -q "$URLP/$repo.git" "$cw/clone" && git -C "$cw/clone" fsck --strict --no-progress >/dev/null 2>&1; then
    while read -r sha; do git -C "$cw/clone" merge-base --is-ancestor "$sha" origin/main && ok=$((ok+1)); done < "$cw/acked"
  fi
  [ "$ok" = "$total" ] || fail "C: after $target only $ok/$total acked commits present"
  md "| $target | ${t_ready}s | $t_push | $ok/$total, fsck clean |"
  echo "   $target: ready after ${t_ready}s, push ok after $t_push, $ok/$total acked commits present"
  # (the forge pods were not restarted: they reconnected to Postgres/MinIO on their own)
done
md
md "forge container restart counts at the end (the phase-B kill -9 crashes restart containers in place): $($K get pods -l app.kubernetes.io/name=forge -o jsonpath='{range .items[*]}{.status.containerStatuses[0].restartCount}{" "}{end}')"
md
md "## Verdict: **$VERDICT**"
echo "==> $VERDICT — wrote $REPORT"
cat "$REPORT"
[ "$VERDICT" = PASS ]

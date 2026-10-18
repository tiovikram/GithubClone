#!/usr/bin/env bash
# Chaos: kill the forge-runner pod that is running an Actions job, mid-job.
#   tests/chaos/runner-kill.sh k3d|kind          (reads data/k8s/<flavor>.env written by up.sh)
#
# For each case a commit is pushed to a repo whose workflow (`on: push`) runs a ~$JOB_SECS s job.
# Once the job is in_progress on a runner pod and $KILL_AFTER s in, that pod is killed:
#   graceful  `kubectl delete pod`: the runner stops taking jobs and drains the running one
#             (FORGE_RUNNER_DRAIN_SECS) -> the job must finish `success`
#   force     `kubectl delete pod --grace-period=0 --force`
#   crash     `kill -9` of the `forge runner` process (the container restarts in place and the
#             runner re-registers under the same name)
# force/crash: the job must not stay stuck. The server's lost-job reaper (scheduler, every 15 s)
# fails jobs without a heartbeat for 3 min -> expected `failure` within ~3.5 min, then
# "re-run failed jobs" must succeed on a surviving/replacement runner. A case FAILs if any run
# is still queued/in_progress after $STUCK_LIMIT s.
# Results: tests/chaos/results/runner-<flavor>-<date>.md
set -uo pipefail
FLAVOR=${1:?usage: runner-kill.sh k3d|kind}
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
# shellcheck disable=SC1090
source "$ROOT/data/k8s/$FLAVOR.env"
BASE=$FORGE_URL; TOKEN=$FORGE_TOKEN
K="kubectl --context $KUBE_CONTEXT -n forge"
CASES=${CASES:-"graceful force crash"}
JOB_SECS=${JOB_SECS:-60}; KILL_AFTER=${KILL_AFTER:-15}; STUCK_LIMIT=${STUCK_LIMIT:-420}
export GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_GLOBAL=/dev/null GIT_TERMINAL_PROMPT=0
STAMP=$(date +%Y%m%d-%H%M%S)
OUT="$ROOT/tests/chaos/results"; RAW="$OUT/raw/runner-$FLAVOR-$STAMP"; mkdir -p "$RAW"
REPORT="$OUT/runner-$FLAVOR-$STAMP.md"
W=$(mktemp -d); trap 'rm -rf "$W"' EXIT
api() { curl -sS -m 20 -H "Authorization: token $TOKEN" -H 'Accept: application/vnd.github+json' "$@"; }
OWNER=$(api "$BASE/api/v3/user" | jq -r .login)
URLP="http://x-access-token:$TOKEN@${BASE#http://}/$OWNER"
VERDICT=PASS
md() { echo "$*" >> "$REPORT"; }
fail() { VERDICT=FAIL; md "- **FAIL**: $*"; echo "FAIL: $*" >&2; }
now() { date +%s; }
repo="chaos-runner-$STAMP"
R="$BASE/api/v3/repos/$OWNER/$repo"

{
  echo "# forge runner chaos — $FLAVOR — $STAMP"
  echo
  echo "- target \`$BASE\`, context \`$KUBE_CONTEXT\`; runner image \`$($K get deploy forge-runner -o jsonpath='{.spec.template.spec.containers[0].image}')\`, replicas $($K get deploy forge-runner -o jsonpath='{.status.readyReplicas}')"
  echo "- job: one step of ${JOB_SECS}s (\`sleep 1\` loop), killed ${KILL_AFTER}s after the job reported in_progress"
  echo "- lost-job reaper: scheduler tick every 15 s, fails in_progress jobs without a heartbeat for 3 min"
  echo
  echo "| case | runner pod killed | killed at (job age) | run concluded after kill | conclusion | expected | reaper log line | re-run | re-run runner | stuck runs |"
  echo "|---|---|---|---|---|---|---|---|---|---|"
} > "$REPORT"

[ "$(api -o /dev/null -w '%{http_code}' -X POST "$BASE/api/v3/user/repos" -d "{\"name\":\"$repo\"}")" = 201 ] || { echo "cannot create $repo" >&2; exit 1; }
src="$W/src"; git init -q -b main "$src"; mkdir -p "$src/.github/workflows"
cat > "$src/.github/workflows/long.yml" <<YAML
name: long
on: push
jobs:
  long:
    runs-on: ubuntu-latest
    timeout-minutes: 10
    steps:
      - name: work
        shell: bash
        run: |
          for i in \$(seq 1 $JOB_SECS); do echo "tick \$i"; sleep 1; done
          echo "finished on \$RUNNER_NAME"
YAML
gitc() { git -C "$src" -c user.name=chaos -c user.email=chaos@forge.local "$@"; }

run_for() { api "$R/actions/runs?head_sha=$1" | jq -c '.workflow_runs[0] // empty'; }
wait_run_completed() { # $1 run id, $2 limit s -> echoes conclusion or "stuck"
  local t0; t0=$(now)
  while [ $(( $(now) - t0 )) -lt "$2" ]; do
    local r; r=$(api "$R/actions/runs/$1")
    [ "$(echo "$r" | jq -r .status)" = completed ] && { echo "$r" | jq -r .conclusion; return; }
    sleep 3
  done
  echo stuck
}
stuck_runs() { # count of non-completed runs in the repo
  local n=0 s
  for s in queued in_progress waiting pending requested; do
    n=$(( n + $(api "$R/actions/runs?status=$s" | jq '.total_count // 0') ))
  done
  echo $n
}
KILL_RUNNER='for d in /proc/[0-9]*; do [ "$(tr "\0" " " < $d/cmdline 2>/dev/null)" = "forge runner " ] && kill -9 ${d#/proc/}; done'

i=0
for case in $CASES; do
  i=$((i+1))
  echo "case $i $case $(date)" > "$src/case.txt"
  gitc add -A; gitc commit -qm "chaos case $i: $case"
  sha=$(git -C "$src" rev-parse HEAD)
  git -C "$src" push -q "$URLP/$repo.git" main || { fail "$case: push failed"; continue; }
  echo "==> case $case ($sha)"
  # wait for the job to start on a runner
  run=""; jid=""; runner=""; t_start=0
  for _ in $(seq 1 120); do
    run=$(run_for "$sha"); rid=$(echo "$run" | jq -r '.id // empty')
    if [ -n "$rid" ]; then
      j=$(api "$R/actions/runs/$rid/jobs" | jq -c '.jobs[0] // empty')
      if [ "$(echo "$j" | jq -r '.status // empty')" = in_progress ] && [ -n "$(echo "$j" | jq -r '.runner_name // empty')" ]; then
        jid=$(echo "$j" | jq -r .id); runner=$(echo "$j" | jq -r .runner_name); t_start=$(now); break
      fi
    fi
    sleep 1
  done
  [ -n "$runner" ] || { fail "$case: job never started on a runner"; continue; }
  sleep "$KILL_AFTER"
  age=$(( $(now) - t_start ))
  t_kill=$(now)
  case $case in
    graceful) $K delete pod "$runner" --wait=false >/dev/null 2>&1; expect=success ;;
    force) $K delete pod "$runner" --grace-period=0 --force >/dev/null 2>&1; expect=failure ;;
    crash) $K exec "$runner" -c runner -- bash -c "$KILL_RUNNER" >/dev/null 2>&1; expect=failure ;;
  esac
  echo "   killed $runner ($case) at job age ${age}s; waiting for the run to conclude"
  concl=$(wait_run_completed "$rid" "$STUCK_LIMIT")
  t_done=$(( $(now) - t_kill ))
  api -L "$R/actions/jobs/$jid/logs" > "$RAW/$case-job.log" 2>/dev/null
  reaper="-"
  grep -q 'lost communication' "$RAW/$case-job.log" && reaper="\"runner has lost communication\""
  [ "$concl" = stuck ] && fail "$case: run $rid still not completed ${STUCK_LIMIT}s after the kill"
  [ "$concl" = "$expect" ] || { [ "$concl" != stuck ] && fail "$case: conclusion $concl, expected $expect"; }
  rerun="-"; rerun_runner="-"
  if [ "$concl" = failure ]; then
    api -o /dev/null -X POST "$R/actions/runs/$rid/rerun-failed-jobs"
    t_r=$(now)
    sleep 2
    rc=$(wait_run_completed "$rid" 300)
    rerun="$rc after $(( $(now) - t_r ))s"
    rerun_runner=$(api "$R/actions/runs/$rid/jobs" | jq -r '.jobs[0].runner_name')
    [ "$rc" = success ] || fail "$case: re-run concluded $rc"
  fi
  # the Deployment must be back at full strength
  $K rollout status deploy/forge-runner --timeout=180s >/dev/null 2>&1 || fail "$case: forge-runner not back to ready"
  stuck=$(stuck_runs)
  [ "$stuck" = 0 ] || fail "$case: $stuck runs not completed"
  md "| $case | $runner | ${age}s | ${t_done}s | $concl | $expect | $reaper | $rerun | $rerun_runner | $stuck |"
  echo "   $case: concluded $concl after ${t_done}s (expected $expect), rerun=$rerun on $rerun_runner, stuck=$stuck"
done

# Final: a fresh push must run to success on the (replaced) runners.
echo "final $(date)" > "$src/case.txt"; gitc add -A; gitc commit -qm final; sha=$(git -C "$src" rev-parse HEAD)
git -C "$src" push -q "$URLP/$repo.git" main
fr=""; for _ in $(seq 1 60); do fr=$(run_for "$sha" | jq -r '.id // empty'); [ -n "$fr" ] && break; sleep 1; done
fc=$(wait_run_completed "$fr" 300)
[ "$fc" = success ] || fail "final run concluded $fc"
md
md "- final push after all kills: run $fr concluded **$fc** on $(api "$R/actions/runs/$fr/jobs" | jq -r '.jobs[0].runner_name')"
md "- runner pods at the end: $($K get pods -l app.kubernetes.io/name=forge-runner -o jsonpath='{range .items[*]}{.metadata.name} ({.status.containerStatuses[0].restartCount} restarts) {end}')"
md "- runs left queued/in_progress in the repo: $(stuck_runs)"
md
md "Job logs: \`tests/chaos/results/raw/runner-$FLAVOR-$STAMP/\`"
md
md "## Verdict: **$VERDICT**"
echo "==> $VERDICT — wrote $REPORT"
cat "$REPORT"
[ "$VERDICT" = PASS ]

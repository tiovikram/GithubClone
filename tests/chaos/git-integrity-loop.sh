#!/usr/bin/env bash
# Continuously commits + pushes to one repo and clones it fresh, verifying integrity, until
# $STOP_FILE exists. Every push that git reported as successful ("acked") must be present in
# every later clone; every clone must pass `git fsck --strict`.
#   git-integrity-loop.sh <base_url> <token> <owner> <repo> <workdir>
# Writes counters to <workdir>/stats (shell vars) and acked SHAs to <workdir>/acked.
set -uo pipefail
BASE=$1; TOKEN=$2; OWNER=$3; REPO=$4; W=$5
STOP_FILE=${STOP_FILE:-$W/stop}
export GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_GLOBAL=/dev/null GIT_TERMINAL_PROMPT=0
# Fail fast on a dead connection instead of hanging (git's default has no timeout).
export GIT_HTTP_LOW_SPEED_LIMIT=1000 GIT_HTTP_LOW_SPEED_TIME=30
# Watchdog (macOS has no `timeout`): stock git can hang forever when the server dies mid-upload.
tmo() {
  perl -e '$t=shift; $p=fork; if(!$p){setpgrp(0,0); exec @ARGV; exit 127}
           $SIG{ALRM}=sub{kill "KILL", -$p; exit 124}; alarm $t; waitpid($p,0); exit($?>>8)' "$@"
}
URL="http://x-access-token:$TOKEN@${BASE#http://}/$OWNER/$REPO.git"
mkdir -p "$W"; : > "$W/acked"; : > "$W/log"
push_ok=0 push_fail=0 clone_ok=0 clone_fail=0 fsck_fail=0 missing=0 iter=0
src="$W/src"; rm -rf "$src"; git init -q -b main "$src"
save() {
  printf 'push_ok=%s\npush_fail=%s\nclone_ok=%s\nclone_fail=%s\nfsck_fail=%s\nmissing=%s\niter=%s\n' \
    $push_ok $push_fail $clone_ok $clone_fail $fsck_fail $missing $iter > "$W/stats"
}
verify_clone() { # $1 = clone dir; checks fsck + all acked commits present
  if ! git -C "$1" fsck --strict --no-progress >/dev/null 2>>"$W/log"; then
    fsck_fail=$((fsck_fail+1)); echo "$(date +%T) FSCK FAILED" >> "$W/log"; fi
  while read -r sha; do
    [ -z "$sha" ] && continue
    if ! git -C "$1" merge-base --is-ancestor "$sha" origin/main 2>/dev/null; then
      missing=$((missing+1)); echo "$(date +%T) MISSING acked commit $sha" >> "$W/log"; fi
  done < "$W/acked"
}
while [ ! -e "$STOP_FILE" ]; do
  iter=$((iter+1))
  # ~40 KB of fresh content per commit + a growing log file, so packs and deltas both occur.
  head -c 30000 /dev/urandom | base64 > "$src/blob-$((iter % 20)).txt"
  echo "iteration $iter $(date +%s)" >> "$src/history.log"
  git -C "$src" add -A
  git -C "$src" -c user.name=chaos -c user.email=chaos@forge.local commit -q -m "chaos $iter"
  sha=$(git -C "$src" rev-parse HEAD)
  if tmo 120 git -C "$src" push -q "$URL" main 2>>"$W/log"; then
    push_ok=$((push_ok+1)); echo "$sha" >> "$W/acked"
  else
    push_fail=$((push_fail+1)); echo "$(date +%T) push $iter failed (unacked $sha)" >> "$W/log"
  fi
  rm -rf "$W/clone"
  if tmo 120 git clone -q "$URL" "$W/clone" 2>>"$W/log"; then
    clone_ok=$((clone_ok+1)); verify_clone "$W/clone"
  else
    clone_fail=$((clone_fail+1)); echo "$(date +%T) clone $iter failed" >> "$W/log"
  fi
  save
done
save

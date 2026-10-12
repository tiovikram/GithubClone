#!/usr/bin/env bash
# Captures the protocol-v2 packet exchange of a stock `git clone` against forge, which is what
# tests/load/git-fetch.js replays. Writes tests/load/fixtures/clone-v2.trace (pack data elided).
#   tests/load/capture-trace.sh <base_url> <token> <owner/repo>
set -euo pipefail
BASE=${1:?base url}; TOKEN=${2:?token}; REPO=${3:?owner/repo}
HERE=$(cd "$(dirname "$0")" && pwd)
export GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_GLOBAL=/dev/null GIT_TERMINAL_PROMPT=0
tmp=$(mktemp -d); trap 'rm -rf "$tmp"' EXIT
hostpath=${BASE#http://}
GIT_TRACE_PACKET=1 git clone -q "http://x-access-token:$TOKEN@$hostpath/$REPO.git" "$tmp/c" 2> "$tmp/trace"
mkdir -p "$HERE/fixtures"
# Strip timestamps/source locations and the binary pack payload.
sed -E 's/^[0-9:.]+ +[a-z_.-]+:[0-9]+ +//' "$tmp/trace" \
  | grep -v -E 'sideband|PACK|^packet: +clone< \\1' \
  | sed -E "s/$TOKEN/<token>/g" > "$HERE/fixtures/clone-v2.trace"
cat "$HERE/fixtures/clone-v2.trace"

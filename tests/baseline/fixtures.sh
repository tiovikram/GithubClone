#!/usr/bin/env bash
# Identical fixtures on either app, created through each app's own API + stock git:
#   loadfix-1..3   120 commits over 60 files (+README.md), 30 open issues with one comment each
#                  (read mix + git clone/fetch)
#   writefix-1..3  auto_init repos the write mix creates issues/comments in
#   fixtures.sh data/bench/<app>.env
set -euo pipefail
# shellcheck disable=SC1090
source "${1:?usage: fixtures.sh data/bench/<app>.env}"
export GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_GLOBAL=/dev/null GIT_TERMINAL_PROMPT=0
case $APP in forgejo) API=$BASE/api/v1;; *) API=$BASE/api/v3;; esac
api() { curl -sS -H "Authorization: token $TOKEN" -H 'Content-Type: application/json' -H 'Accept: application/json' "$@"; }
tmp=$(mktemp -d); trap 'rm -rf "$tmp"' EXIT
# Deterministic content: the same bytes are pushed to both apps.
for i in 1 2 3; do
  name="loadfix-$i"
  code=$(api -o /dev/null -w '%{http_code}' -X POST "$API/user/repos" -d "{\"name\":\"$name\",\"description\":\"k6 fixture\"}")
  if [ "$code" != 201 ]; then echo "   $OWNER/$name exists (HTTP $code), reusing"; continue; fi
  src="$tmp/$name"; git init -q -b main "$src"
  printf '# %s\n\nk6 load fixture\n' "$name" > "$src/README.md"
  for c in $(seq 1 120); do
    f=$((c % 60))
    { echo "commit $c of fixture $i"
      awk -v s="$i$c" 'BEGIN{srand(s); for(k=0;k<32;k++){l=""; for(j=0;j<32;j++) l=l sprintf("%02x",int(rand()*256)); print l}}'
    } > "$src/file$f.txt"
    git -C "$src" add -A
    GIT_AUTHOR_DATE="2026-01-01T00:00:${c}Z" GIT_COMMITTER_DATE="2026-01-01T00:00:${c}Z" \
      git -C "$src" -c user.name=k6 -c user.email=k6@bench.local commit -q -m "fixture commit $c"
  done
  git -C "$src" push -q "http://x-access-token:$TOKEN@${BASE#http://}/$OWNER/$name.git" main
  for n in $(seq 1 30); do
    num=$(api -X POST "$API/repos/$OWNER/$name/issues" -d "{\"title\":\"fixture issue $n\",\"body\":\"Body of fixture issue $n with some **markdown**.\"}" | jq -r .number)
    api -o /dev/null -X POST "$API/repos/$OWNER/$name/issues/$num/comments" -d '{"body":"fixture comment"}'
  done
  echo "   created $OWNER/$name ($(git -C "$src" rev-parse --short HEAD), 30 issues)"
done
for i in 1 2 3; do
  api -o /dev/null -w "   writefix-$i: HTTP %{http_code}\n" -X POST "$API/user/repos" -d "{\"name\":\"writefix-$i\",\"auto_init\":true}"
done

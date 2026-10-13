#!/usr/bin/env bash
# Stock git smart-HTTP check, identical for any server: create an empty repo through the server's
# own API, push, clone, fetch, push from a second clone, branch create/delete, tag push,
# protocol v2 advertisement. Prints one line per check and "git smoke: PASS n/N".
#   git-smoke.sh <base_url> <api_root> <owner> <token>
#   e.g. git-smoke.sh http://localhost:3320 http://localhost:3320/api/v1 conf <token>
set -uo pipefail
BASE=${1%/}; API=${2%/}; OWNER=$3; TOKEN=$4
export GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_GLOBAL=/dev/null GIT_TERMINAL_PROMPT=0
W=$(mktemp -d); trap 'rm -rf "$W"' EXIT
REPO="gitsmoke-$(date +%s)-$RANDOM"
URL="${BASE%%://*}://x-access-token:$TOKEN@${BASE#*://}/$OWNER/$REPO.git"
PASS=0; TOTAL=0
ck() { # name cmd...
  local name=$1; shift; TOTAL=$((TOTAL+1))
  if "$@" >>"$W/log" 2>&1; then PASS=$((PASS+1)); echo "PASS  $name"; else echo "FAIL  $name"; fi
}
g() { git -c user.name=smoke -c user.email=smoke@example.com "$@"; }
create() {
  [ "$(curl -sS -o /dev/null -w '%{http_code}' -H "Authorization: token $TOKEN" -H 'Content-Type: application/json' \
       -X POST "$API/user/repos" -d "{\"name\":\"$REPO\"}")" = 201 ]
}
seed() {
  g init -q -b main "$W/src" && for i in 1 2 3 4 5; do
    head -c 20000 /dev/urandom | base64 > "$W/src/f$i.txt"; g -C "$W/src" add -A; g -C "$W/src" commit -qm "c$i" || return 1
  done
}
same_head() { [ "$(git -C "$1" rev-parse HEAD)" = "$(git -C "$2" rev-parse "${3:-HEAD}")" ]; }
v2adv() {
  curl -sS -u "x-access-token:$TOKEN" -H 'Git-Protocol: version=2' "$BASE/$OWNER/$REPO.git/info/refs?service=git-upload-pack" \
    | grep -a -q 'version 2'
}

ck "create repo via API (POST user/repos -> 201)" create
ck "local repo with 5 commits" seed
ck "git push (new repo, main)" g -C "$W/src" push -q "$URL" main
ck "git ls-remote shows main" bash -c "git ls-remote '$URL' | grep -q refs/heads/main"
ck "git clone" git clone -q "$URL" "$W/a"
ck "clone HEAD == pushed HEAD" same_head "$W/a" "$W/src"
ck "git fsck --strict on clone" git -C "$W/a" fsck --strict --no-progress
ck "second clone" git clone -q "$URL" "$W/b"
ck "commit + push from clone A" bash -c "echo more >> '$W/a/f1.txt' && git -C '$W/a' -c user.name=s -c user.email=s@x commit -qam more && git -C '$W/a' push -q origin main"
ck "git fetch in clone B" git -C "$W/b" fetch -q origin
ck "fetched origin/main == A's HEAD" same_head "$W/a" "$W/b" origin/main
ck "push new branch" g -C "$W/a" push -q origin main:refs/heads/topic
ck "push tag" bash -c "git -C '$W/a' tag v1 && git -C '$W/a' push -q origin v1"
ck "delete remote branch" g -C "$W/a" push -q origin :refs/heads/topic
ck "non-fast-forward push rejected" bash -c "! git -C '$W/b' -c user.name=s -c user.email=s@x commit -q --allow-empty -m x >/dev/null 2>&1 || ! git -C '$W/b' push -q origin HEAD:main"
ck "shallow clone (--depth 1)" git clone -q --depth 1 "$URL" "$W/shallow"
ck "protocol v2 advertised" v2adv
echo "git smoke: PASS $PASS/$TOTAL ($REPO)"
[ "$PASS" = "$TOTAL" ]

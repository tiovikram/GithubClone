#!/usr/bin/env bash
# gh compatibility suite: runs the stock GitHub CLI against a fresh forge server configured as a
# GitHub Enterprise Server host (`forge.localhost`), as recommended by docs/research/gh-protocol.md
# §1.6: gh talks plain HTTP over a unix socket (`http_unix_socket`), git talks to forge's TCP
# port through a test-only `url.insteadOf` rewrite and a static credential helper.
#
# Prints a per-command table and `PASS n/N (pct%)`, and writes tests/gh/report.md.
#
# Environment knobs:
#   GH                 gh binary (default /opt/homebrew/bin/gh)
#   FORGE_GH_PORT      TCP port for git smart HTTP (default 3104)
#   DATABASE_URL       Postgres database (default postgres://forge:forge@localhost/forge_graphql)
#   SKIP_BUILD=1       don't run cargo build
#   KEEP=1             keep the work directory and leave the server running
#   VERBOSE=1          print each command's output
#   GH_SUITE_EXTERNAL=1  don't build/start forge; test an already running server (used by
#                      tests/baseline/compat.sh to run this same suite against Forgejo). Needs
#                      GH_SUITE_HOST (host name in the server's URLs), GH_SUITE_SOCK (unix socket
#                      forwarded to the server; gh speaks plain HTTP over it), FORGE_GH_PORT (TCP
#                      port git uses as http://GH_SUITE_HOST:port), GH_SUITE_ALICE, GH_SUITE_BOB,
#                      GH_SUITE_TOKEN, GH_SUITE_BOB_TOKEN (two existing users and their tokens).
#                      Optional: GH_SUITE_WORK, GH_SUITE_REPORT.
set -uo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
GH="${GH:-/opt/homebrew/bin/gh}"
PORT="${FORGE_GH_PORT:-3104}"
export PATH="/opt/homebrew/opt/rustup/bin:/opt/homebrew/opt/postgresql@16/bin:$HOME/.cargo/bin:$PATH"
export DATABASE_URL="${DATABASE_URL:-postgres://forge:forge@localhost/forge_graphql}"
HOST="${GH_SUITE_HOST:-forge.localhost}"

WORK="${GH_SUITE_WORK:-$ROOT/data/gh-suite}"
SOCK="${GH_SUITE_SOCK:-/tmp/forge-gh-$PORT.sock}"   # macOS sun_path is limited to 104 bytes
rm -rf "$WORK"
mkdir -p "$WORK"/{logs,gh,xdg-cache,xdg-config,xdg-state,work}

# ------------------------------------------------------------------------------------------------
# Build and start forge

if [ "${GH_SUITE_EXTERNAL:-}" = "1" ]; then
  ALICE="$GH_SUITE_ALICE"; BOB="$GH_SUITE_BOB"; TOKEN="$GH_SUITE_TOKEN"; BOB_TOKEN="$GH_SUITE_BOB_TOKEN"
  [ -S "$SOCK" ] || { echo "GH_SUITE_SOCK $SOCK is not a socket" >&2; exit 2; }
else
if [ "${SKIP_BUILD:-}" != "1" ]; then
  (cd "$ROOT" && cargo build -q -p forge) || { echo "build failed" >&2; exit 2; }
fi
FORGE="$ROOT/target/debug/forge"

pkill -f "forge-gh-suite-$PORT" 2>/dev/null || true
lsof -ti "tcp:$PORT" 2>/dev/null | xargs kill 2>/dev/null || true
rm -f "$SOCK"

export FORGE_LISTEN="127.0.0.1:$PORT"
export FORGE_BASE_URL="https://$HOST"
export FORGE_UNIX_SOCKET="$SOCK"
export FORGE_STORAGE="file://$WORK/objects"
export FORGE_CACHE_DIR="$WORK/cache"
export RUST_LOG="${RUST_LOG:-forge=info}"
# The argv[0] marker lets us find (and kill) exactly this server.
(exec -a "forge-gh-suite-$PORT" "$FORGE" serve >"$WORK/server.log" 2>&1) &
SERVER_PID=$!
cleanup() {
  if [ "${KEEP:-}" != "1" ]; then
    kill "$SERVER_PID" 2>/dev/null
    rm -f "$SOCK"
  fi
}
trap cleanup EXIT
for _ in $(seq 1 100); do
  curl -sf "http://127.0.0.1:$PORT/healthz" >/dev/null 2>&1 && [ -S "$SOCK" ] && break
  sleep 0.2
done
curl -sf "http://127.0.0.1:$PORT/healthz" >/dev/null || { echo "forge failed to start; see $WORK/server.log" >&2; exit 2; }

SUFFIX="$(LC_ALL=C tr -dc 'a-z0-9' </dev/urandom | head -c 6)"
ALICE="alice-$SUFFIX"
BOB="bob-$SUFFIX"
"$FORGE" admin create-user --login "$ALICE" --password "password123" --email "$ALICE@example.com" >/dev/null
"$FORGE" admin create-user --login "$BOB" --password "password123" --email "$BOB@example.com" >/dev/null
TOKEN="$("$FORGE" admin create-token --login "$ALICE" | tail -1)"
BOB_TOKEN="$("$FORGE" admin create-token --login "$BOB" | tail -1)"
fi

# ------------------------------------------------------------------------------------------------
# Isolated gh + git configuration (never touches ~/.config/gh, ~/.gitconfig or the keychain)

export GH_CONFIG_DIR="$WORK/gh"
export XDG_CACHE_HOME="$WORK/xdg-cache" XDG_CONFIG_HOME="$WORK/xdg-config" XDG_STATE_HOME="$WORK/xdg-state"
export GH_HOST="$HOST" GH_ENTERPRISE_TOKEN="$TOKEN"
export GH_TELEMETRY=disabled GH_NO_UPDATE_NOTIFIER=1 GH_NO_EXTENSION_UPDATE_NOTIFIER=1 GH_PROMPT_DISABLED=1
export GH_PAGER=cat PAGER=cat NO_COLOR=1 CLICOLOR=0 GH_SPINNER_DISABLED=1
unset GH_TOKEN GITHUB_TOKEN GITHUB_ENTERPRISE_TOKEN GH_REPO
cat >"$GH_CONFIG_DIR/config.yml" <<EOF
version: "1"
git_protocol: https
prompt: disabled
http_unix_socket: $SOCK
EOF
cat >"$GH_CONFIG_DIR/hosts.yml" <<EOF
$HOST:
    git_protocol: https
    user: $ALICE
EOF

export GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_GLOBAL="$WORK/gitconfig" GIT_TERMINAL_PROMPT=0
cat >"$GIT_CONFIG_GLOBAL" <<EOF
[user]
	name = Alice Test
	email = $ALICE@example.com
[init]
	defaultBranch = main
[url "http://$HOST:$PORT/"]
	insteadOf = https://$HOST/
[credential "http://$HOST:$PORT"]
	helper =
	helper = "!f() { echo username=x-access-token; echo password=$TOKEN; }; f"
[advice]
	detachedHead = false
EOF

# ------------------------------------------------------------------------------------------------
# Test runner

R="$HOST/$ALICE/demo"      # main test repository
NAMES=(); RESULTS=(); CODES=(); NOTES=(); CMDS=()
PASS=0; TOTAL=0
CASE_DIR="$WORK/work"

# t NAME EXPECTED_EXIT REGEX CMD...   (REGEX is an extended regex over stdout+stderr; "" = any)
t() {
  local name="$1" want="$2" re="$3"; shift 3
  TOTAL=$((TOTAL + 1))
  local log; log="$WORK/logs/$(printf '%03d' "$TOTAL").log"
  (cd "$CASE_DIR" && "$@") >"$log" 2>&1 </dev/null
  local code=$?
  local ok=1 note=""
  if [ "$code" != "$want" ]; then ok=0; note="exit $code (want $want)"; fi
  if [ $ok = 1 ] && [ -n "$re" ] && ! grep -Eq -- "$re" "$log"; then ok=0; note="output did not match /$re/"; fi
  if [ $ok = 0 ]; then
    local first; first="$(grep -v '^\s*$' "$log" | head -3 | tr '\n' ' ' | cut -c1-160)"
    note="$note: $first"
  fi
  NAMES+=("$name"); CODES+=("$code"); NOTES+=("$note"); CMDS+=("$*")
  if [ $ok = 1 ]; then PASS=$((PASS + 1)); RESULTS+=("PASS"); else RESULTS+=("FAIL"); fi
  if [ "${VERBOSE:-}" = "1" ]; then echo "== [$([ $ok = 1 ] && echo PASS || echo FAIL)] $name"; cat "$log"; fi
}

as_bob() { GH_ENTERPRISE_TOKEN="$BOB_TOKEN" "$@"; }
git_q() { git "$@" >/dev/null 2>&1; }

# ------------------------------------------------------------------------------------------------
# auth / api

t "auth status"                      0 "Logged in to $HOST"        "$GH" auth status --hostname "$HOST"
t "api user (REST GET)"              0 "\"login\": *\"$ALICE\""      "$GH" api user
t "api graphql query"                0 "\"login\": *\"$ALICE\""      "$GH" api graphql -f query='query { viewer { login } }'

# ------------------------------------------------------------------------------------------------
# repositories

t "repo create (GraphQL)"            0 "https://$HOST/$ALICE/demo"  "$GH" repo create demo --public --description "Demo repo"
t "repo create --add-readme (REST)"  0 "https://$HOST/$ALICE/readme" "$GH" repo create readme --private --add-readme
t "repo view"                        0 "name:.*$ALICE/demo"          "$GH" repo view "$R"
t "repo view (README)"               0 "# readme"                    "$GH" repo view "$HOST/$ALICE/readme"
t "repo view --json"                 0 "\"visibility\": *\"PUBLIC\"" "$GH" repo view "$R" --json name,nameWithOwner,description,visibility,isPrivate,owner,url,defaultBranchRef,hasIssuesEnabled,stargazerCount,forkCount,viewerPermission
t "repo list"                        0 "$ALICE/demo"                 "$GH" repo list
t "repo list OWNER --json"           0 "\"nameWithOwner\": *\"$ALICE/readme\"" "$GH" repo list "$ALICE" --json nameWithOwner,isPrivate,visibility,isFork,description
t "repo edit"                        0 ""                            "$GH" repo edit "$R" --description "Edited description" --homepage "https://example.com"
t "repo view after edit"             0 "Edited description"          "$GH" repo view "$R" --json description,homepageUrl

# Bob becomes a collaborator (so he can be requested as reviewer / assigned), then seed the demo
# repo over git (setup, not gh tests).
"$GH" api -X PUT "repos/$ALICE/demo/collaborators/$BOB" -f permission=push >/dev/null 2>&1
SEED="$WORK/seed"
git_q clone "https://$HOST/$ALICE/demo.git" "$SEED"
( cd "$SEED" && echo "# demo" >README.md && git add . && git commit -qm "Initial commit" && git push -q origin HEAD:main ) >/dev/null 2>&1
for b in feature feature2 feature3 feature4; do
  ( cd "$SEED" && git checkout -q -b "$b" main && echo "$b change" >"$b.txt" && git add . && git commit -qm "Add $b" \
      && echo "$b more" >>"$b.txt" && git commit -qam "Extend $b" && git push -q origin "$b" ) >/dev/null 2>&1
done

t "repo clone"                       0 ""                            "$GH" repo clone "$ALICE/demo" "$WORK/work/clone"
t "repo clone result"                0 "feature4"                    git -C "$WORK/work/clone" branch -r
t "repo fork (as other user)"        0 "$BOB/demo"                   as_bob "$GH" repo fork "$R" --clone=false
t "repo list --fork (other user)"    0 "$BOB/demo"                   as_bob "$GH" repo list "$BOB" --fork

# ------------------------------------------------------------------------------------------------
# labels

t "label create (REST)"              0 ""                            "$GH" label create triage --color ff0000 --description "Needs triage" -R "$R"
t "label create --force (REST)"      0 ""                            "$GH" label create triage --color 00ff00 --description "Updated" --force -R "$R"
REPO_ID="$("$GH" api graphql -f query="query { repository(owner: \"$ALICE\", name: \"demo\") { id } }" --jq .data.repository.id 2>/dev/null)"
t "api graphql mutation (createLabel)" 0 "\"name\": *\"gql-label\"" "$GH" api graphql -f query='mutation($r: ID!) { createLabel(input: {repositoryId: $r, name: "gql-label", color: "a2eeef"}) { label { name color } } }' -f r="$REPO_ID"
t "label list"                       0 "triage"                 "$GH" label list -R "$R"
t "label list --json"                0 "\"name\": *\"bug\""          "$GH" label list -R "$R" --json name,color,description

# ------------------------------------------------------------------------------------------------
# issues

t "issue create"                     0 "https://$HOST/$ALICE/demo/issues/1" "$GH" issue create -R "$R" -t "First bug" -b "Steps to reproduce"
t "issue create with metadata"       0 "/issues/2"                   "$GH" issue create -R "$R" -t "Second bug" -b "More" -l bug -a "$ALICE"
t "issue create (to delete)"         0 "/issues/3"                   "$GH" issue create -R "$R" -t "Throwaway" -b "x"
t "issue list"                       0 "First bug"                   "$GH" issue list -R "$R"
t "issue list --json"                0 "\"title\": *\"Second bug\""  "$GH" issue list -R "$R" --json number,title,labels,assignees,author,state,createdAt,url,comments,milestone,body,closed,updatedAt,id,reactionGroups,stateReason
t "issue list --label (search)"      0 "Second bug"                  "$GH" issue list -R "$R" --label bug
t "issue list --search"              0 "First bug"                   "$GH" issue list -R "$R" --search "First"
t "issue list --assignee"            0 "Second bug"                  "$GH" issue list -R "$R" --assignee "$ALICE"
t "issue view"                       0 "First bug"                   "$GH" issue view 1 -R "$R"
t "issue comment"                    0 "#issuecomment-"              "$GH" issue comment 1 -R "$R" -b "A comment"
t "issue view --comments"            0 "A comment"                   "$GH" issue view 1 -R "$R" --comments
t "issue view --json"                0 "\"body\": *\"A comment\""    "$GH" issue view 1 -R "$R" --json number,title,body,state,author,labels,assignees,comments,milestone,reactionGroups,url
t "issue edit"                       0 "/issues/1"                   "$GH" issue edit 1 -R "$R" --title "First bug (edited)" --add-label bug --add-label enhancement --add-assignee "$ALICE"
t "issue edit --remove-label"        0 "/issues/1"                   "$GH" issue edit 1 -R "$R" --remove-label enhancement
t "issue view after edit"            0 "First bug \\(edited\\)"      "$GH" issue view 1 -R "$R" --json title,labels,assignees
t "issue close"                      0 "Closed issue"                "$GH" issue close 1 -R "$R" --reason "not planned" -c "Closing"
t "issue list --state closed"        0 "First bug"                   "$GH" issue list -R "$R" --state closed
t "issue reopen"                     0 "Reopened issue"              "$GH" issue reopen 1 -R "$R"
t "issue status"                     0 "Second bug"                  "$GH" issue status -R "$R"
t "issue delete"                     0 ""                            "$GH" issue delete 3 -R "$R" --yes
t "issue view deleted (404)"         1 "Could not resolve"           "$GH" issue view 3 -R "$R"

# ------------------------------------------------------------------------------------------------
# pull requests

t "pr create"                        0 "https://$HOST/$ALICE/demo/pull/4" "$GH" pr create -R "$R" --head feature --base main -t "Add feature" -b "Feature body"
t "pr create (reviewer, label, assignee)" 0 "/pull/5"                "$GH" pr create -R "$R" --head feature2 --base main -t "Add feature2" -b "Body 2" -r "$BOB" -l bug -a "$ALICE"
t "pr create --draft"                0 "/pull/6"                     "$GH" pr create -R "$R" --head feature3 --base main -t "Add feature3" -b "Body 3" --draft
t "pr create (4th)"                  0 "/pull/7"                     "$GH" pr create -R "$R" --head feature4 --base main -t "Add feature4" -b "Body 4"
t "pr create duplicate fails"        1 "already exists"              "$GH" pr create -R "$R" --head feature --base main -t "Dup" -b "x"
t "pr list"                          0 "Add feature2"                "$GH" pr list -R "$R"
t "pr list --json"                   0 "\"headRefName\": *\"feature\"" "$GH" pr list -R "$R" --json number,title,headRefName,baseRefName,state,author,labels,isDraft,url,reviewDecision,mergeable,additions,deletions,reviewRequests,assignees,commits,files,statusCheckRollup,latestReviews
t "pr list --author (search)"        0 "Add feature"                 "$GH" pr list -R "$R" --author "$ALICE"
t "pr list --draft (search)"         0 "Add feature3"                "$GH" pr list -R "$R" --draft
t "pr view"                          0 "Add feature"                 "$GH" pr view 4 -R "$R"
t "pr view --json"                   0 "\"mergeable\": *\"MERGEABLE\"" "$GH" pr view 4 -R "$R" --json number,title,state,body,author,mergeable,mergeStateStatus,additions,deletions,changedFiles,commits,files,headRefName,baseRefName,isDraft,reviewRequests,reviews,statusCheckRollup,url
t "pr view reviewers"                0 "$BOB"                        "$GH" pr view 5 -R "$R" --json reviewRequests,labels,assignees
t "pr diff"                          0 "diff --git a/feature.txt"     "$GH" pr diff 4 -R "$R"
t "pr diff --name-only"              0 "feature.txt"                 "$GH" pr diff 4 -R "$R" --name-only
t "pr comment"                       0 "#issuecomment-"              "$GH" pr comment 4 -R "$R" -b "PR comment"
t "pr review --comment"              0 ""                            "$GH" pr review 4 -R "$R" --comment -b "Looks reasonable"
t "pr review --approve (other user)" 0 ""                            as_bob "$GH" pr review 4 -R "$R" --approve
t "pr review --request-changes (other user)" 0 ""                    as_bob "$GH" pr review 5 -R "$R" --request-changes -b "Please fix"
t "pr view review decision"          0 "APPROVED"                    "$GH" pr view 4 -R "$R" --json reviewDecision,latestReviews
t "pr edit"                          0 "/pull/4"                     "$GH" pr edit 4 -R "$R" --title "Add feature (edited)" --add-label enhancement
t "pr ready --undo"                  0 ""                            "$GH" pr ready 7 -R "$R" --undo
t "pr ready"                         0 ""                            "$GH" pr ready 6 -R "$R"
t "pr view after ready"              0 "\"isDraft\": *false"         "$GH" pr view 6 -R "$R" --json isDraft
t "pr ready (undo the undo)"         0 ""                            "$GH" pr ready 7 -R "$R"

# commit status for `pr checks` (REST statuses API; falls back to SQL if not implemented)
HEAD_SHA="$(git -C "$SEED" rev-parse feature)"
if ! "$GH" api "repos/$ALICE/demo/statuses/$HEAD_SHA" -f state=success -f context=ci/test -f description="All good" >/dev/null 2>&1 \
    && [ "${GH_SUITE_EXTERNAL:-}" != "1" ]; then
  psql "$DATABASE_URL" -q -c "INSERT INTO commit_statuses (repo_id, sha, state, context, description)
     SELECT r.id, '$HEAD_SHA', 'success', 'ci/test', 'All good' FROM repositories r JOIN users u ON u.id = r.owner_id
      WHERE u.login = '$ALICE' AND r.name = 'demo'" >/dev/null 2>&1
fi
t "pr checks"                        0 "ci/test"                     "$GH" pr checks 4 -R "$R"

CASE_DIR="$WORK/work/clone"
t "pr checkout"                      0 ""                            "$GH" pr checkout 4
t "pr checkout result"               0 "^feature$"                   git branch --show-current
t "pr status"                        0 "Add feature"                 "$GH" pr status
t "pr view (current branch)"         0 "Add feature"                 "$GH" pr view
t "pr merge --merge --delete-branch" 0 ""                            "$GH" pr merge 4 --merge --delete-branch
t "pr merge result (local main)"     0 "Merge pull request #4"       git log -1 --format=%s main
CASE_DIR="$WORK/work"
t "pr view merged"                   0 "\"state\": *\"MERGED\""      "$GH" pr view 4 -R "$R" --json state,mergedAt,mergedBy,mergeCommit
t "pr close"                         0 "Closed pull request"         "$GH" pr close 5 -R "$R" -c "Closing for now"
t "pr reopen"                        0 "Reopened pull request"       "$GH" pr reopen 5 -R "$R"
t "pr merge --squash"                0 ""                            "$GH" pr merge 6 -R "$R" --squash
t "pr merge --rebase"                0 ""                            "$GH" pr merge 7 -R "$R" --rebase
t "pr list --state merged"           0 "Add feature4"                "$GH" pr list -R "$R" --state merged
t "pr merge result (remote log)"     0 "Add feature3 \\(#6\\)"       "$GH" api "repos/$ALICE/demo/commits?per_page=10" --jq '.[].commit.message'

# ------------------------------------------------------------------------------------------------
# releases (REST releases API)

t "release create"                   0 "/releases/tag/v1.0"          "$GH" release create v1.0 -R "$R" --title "v1.0" --notes "First release"
t "release list"                     0 "v1.0"                        "$GH" release list -R "$R"
t "release view"                     0 "First release"               "$GH" release view v1.0 -R "$R"

# ------------------------------------------------------------------------------------------------
# gh api: POST with fields, pagination, repo delete

t "api POST -f (REST)"               0 "\"full_name\": *\"$ALICE/apirepo\"" "$GH" api user/repos -f name=apirepo -f description="made by gh api"
t "api --paginate (REST)"            0 "$ALICE/apirepo"              "$GH" api --paginate "user/repos?per_page=1" --jq '.[].full_name'
t "api graphql --paginate"           0 "apirepo"                     "$GH" api graphql --paginate -f query='query($endCursor: String) { viewer { repositories(first: 1, after: $endCursor) { nodes { name } pageInfo { hasNextPage endCursor } } } }'
t "repo delete"                      0 ""                            "$GH" repo delete "$ALICE/apirepo" --yes
t "repo view deleted (404)"          1 "Could not resolve"           "$GH" repo view "$ALICE/apirepo"

# ------------------------------------------------------------------------------------------------
# Report

PCT=$(( TOTAL > 0 ? PASS * 100 / TOTAL : 0 ))
REPORT="${GH_SUITE_REPORT:-$ROOT/tests/gh/report.md}"
{
  echo "# gh compatibility report"
  echo
  echo "gh: \`$("$GH" --version | head -1)\`, host \`$HOST\` (GHES path, http_unix_socket). Generated $(date -u +%Y-%m-%dT%H:%M:%SZ)."
  echo
  echo "**PASS $PASS/$TOTAL ($PCT%)**"
  echo
  echo "| # | Case | Result | Exit | Notes |"
  echo "|---|------|--------|------|-------|"
  for i in "${!NAMES[@]}"; do
    note="${NOTES[$i]//|/\\|}"
    echo "| $((i + 1)) | ${NAMES[$i]} | ${RESULTS[$i]} | ${CODES[$i]} | ${note} |"
  done
} >"$REPORT"

printf '%-4s %-42s %-5s %s\n' "#" "case" "res" "notes"
for i in "${!NAMES[@]}"; do
  printf '%-4s %-42s %-5s %s\n' "$((i + 1))" "${NAMES[$i]}" "${RESULTS[$i]}" "${NOTES[$i]}"
done
echo
echo "PASS $PASS/$TOTAL ($PCT%)"
echo "logs: $WORK/logs   report: $REPORT"
[ "$PASS" = "$TOTAL" ]

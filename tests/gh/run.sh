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
mkdir -p "$WORK"/{logs,gh,xdg-cache,xdg-config,xdg-state,xdg-data,work}

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
export XDG_CACHE_HOME="$WORK/xdg-cache" XDG_CONFIG_HOME="$WORK/xdg-config" XDG_STATE_HOME="$WORK/xdg-state" XDG_DATA_HOME="$WORK/xdg-data"
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
# For debugging with KEEP=1: `source data/gh-suite/env.sh` gives the same gh/git environment.
cat >"$WORK/env.sh" <<EOF
export GH_CONFIG_DIR="$GH_CONFIG_DIR" XDG_CACHE_HOME="$XDG_CACHE_HOME" XDG_CONFIG_HOME="$XDG_CONFIG_HOME"
export XDG_STATE_HOME="$XDG_STATE_HOME" XDG_DATA_HOME="$XDG_DATA_HOME"
export GH_HOST="$HOST" GH_ENTERPRISE_TOKEN="$TOKEN" GH_TELEMETRY=disabled GH_NO_UPDATE_NOTIFIER=1 GH_PROMPT_DISABLED=1
export GH_PAGER=cat NO_COLOR=1 GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_GLOBAL="$GIT_CONFIG_GLOBAL" GIT_TERMINAL_PROMPT=0
ALICE="$ALICE" BOB="$BOB" TOKEN="$TOKEN" BOB_TOKEN="$BOB_TOKEN" R="$HOST/$ALICE/demo"
EOF

# ------------------------------------------------------------------------------------------------
# Test runner

R="$HOST/$ALICE/demo"      # main test repository
NAMES=(); RESULTS=(); CODES=(); NOTES=(); CMDS=(); KEYS=()
PASS=0; TOTAL=0; LASTKEY=""
CASE_DIR="$WORK/work"

# The gh command a case exercises: $K if set, else derived from the words after the gh binary
# (`gh2` counts as gh), else the previous case's command (follow-up checks like "clone result").
cmd_key() {
  if [ -n "${K:-}" ]; then echo "$K"; return; fi
  local i=0 args=("$@") w=()
  while [ $i -lt ${#args[@]} ] && [ "${args[$i]}" != "$GH" ] && [ "${args[$i]}" != gh2 ]; do i=$((i + 1)); done
  if [ $i -ge ${#args[@]} ]; then echo "$LASTKEY"; return; fi
  i=$((i + 1))
  while [ $i -lt ${#args[@]} ] && [ ${#w[@]} -lt 3 ]; do
    case "${args[$i]}" in -*|*/*|*[[:upper:][:digit:].]*) break;; esac
    w+=("${args[$i]}"); i=$((i + 1))
  done
  case "${w[0]:-}" in
    api|browse|status|completion|licenses|copilot) echo "${w[0]}";;
    repo) case "${w[1]:-}" in deploy-key|autolink|gitignore|license) echo "${w[*]:0:3}";; *) echo "${w[*]:0:2}";; esac;;
    *) echo "${w[*]:0:2}";;
  esac
}

# t NAME EXPECTED_EXIT REGEX CMD...   (REGEX is an extended regex over stdout+stderr; "" = any)
t() {
  local name="$1" want="$2" re="$3"; shift 3
  local key; key="$(cmd_key "$@")"; LASTKEY="$key"
  TOTAL=$((TOTAL + 1))
  local log; log="$WORK/logs/$(printf '%03d' "$TOTAL").log"
  (cd "$CASE_DIR" && "$@") >"$log" 2>&1 </dev/null &
  local pid=$!
  # Watchdog: a case that hangs (e.g. waiting for a browser) is killed after CASE_TIMEOUT seconds.
  ( sleep "${CASE_TIMEOUT:-60}"; pkill -TERM -P "$pid" 2>/dev/null; kill -TERM "$pid" 2>/dev/null; echo "[timed out]" >>"$log" ) &
  local wd=$!
  wait "$pid"
  local code=$?
  pkill -P "$wd" 2>/dev/null; kill "$wd" 2>/dev/null; wait "$wd" 2>/dev/null
  local ok=1 note=""
  if [ "$code" != "$want" ]; then ok=0; note="exit $code (want $want)"; fi
  if [ $ok = 1 ] && [ -n "$re" ] && ! grep -Eq -- "$re" "$log"; then ok=0; note="output did not match /$re/"; fi
  if [ $ok = 0 ]; then
    local first; first="$(grep -v '^\s*$' "$log" | head -3 | tr '\n' ' ' | cut -c1-160)"
    note="$note: $first"
  fi
  NAMES+=("$name"); CODES+=("$code"); NOTES+=("$note"); CMDS+=("$*"); KEYS+=("$key")
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


# ================================================================================================
# Extended coverage: every category-(a) command of tests/gh/commands.tsv (see docs/gh-coverage.md)
# has at least one case below (or above). Cases whose gh command can't be derived from argv set K.

last_log() { printf '%s/logs/%03d.log' "$WORK" "$TOTAL"; }
last_num() { grep -Eo '/(pull|issues)/[0-9]+' "$(last_log)" | tail -1 | grep -Eo '[0-9]+$'; }
api_q() { "$GH" api "$@" 2>/dev/null; }

# A second, token-less gh config for the auth commands (login/switch/logout/setup-git). Tokens are
# stored with --insecure-storage in this file; gh still probes the OS keyring for
# `gh:forge.localhost` entries (read/delete only, never write), exactly as `gh auth status` does.
GH2_DIR="$WORK/gh2"
mkdir -p "$GH2_DIR"
cp "$GH_CONFIG_DIR/config.yml" "$GH2_DIR/config.yml"
: >"$WORK/gitconfig2"
gh2() { env -u GH_ENTERPRISE_TOKEN GH_CONFIG_DIR="$GH2_DIR" GIT_CONFIG_GLOBAL="$WORK/gitconfig2" "$GH" "$@"; }
login_as() { printf '%s\n' "$1" | gh2 auth login --hostname "$HOST" --with-token --git-protocol https --insecure-storage; }
# `gh auth refresh` runs the OAuth device flow: wait for the one-time code gh prints, approve it
# as the signed-in user (what the /login/device page does), and let gh finish polling.
refresh_via_device_flow() {
  local out="$WORK/refresh.out" code="" pid i
  gh2 auth refresh --hostname "$HOST" --scopes read:project --insecure-storage --clipboard=false >"$out" 2>&1 </dev/null &
  pid=$!
  for i in $(seq 1 100); do
    code="$(grep -Eo '[A-Z0-9]{4}-[A-Z0-9]{4}' "$out" | head -1)"
    [ -n "$code" ] && break
    sleep 0.1
  done
  [ -n "$code" ] && curl -s -o /dev/null --unix-socket "$SOCK" -X POST -H "Authorization: token $TOKEN" \
    -H 'Content-Type: application/json' -d "{\"user_code\":\"$code\"}" "http://$HOST/api/forge/device/approve"
  wait "$pid"; local rc=$?
  cat "$out"
  return $rc
}

# More content on main: a workflow (for gh workflow/run) and a docs/ directory (for read-dir).
( cd "$SEED" && git fetch -q origin && git checkout -q -B main origin/main \
    && mkdir -p .github/workflows docs \
    && printf 'name: CI\non:\n  push:\n  workflow_dispatch:\njobs:\n  build:\n    runs-on: ubuntu-latest\n    steps:\n      - run: echo hi\n' >.github/workflows/ci.yml \
    && echo "# Guide" >docs/guide.md && git add . && git commit -qm "Add CI workflow and docs" && git push -q origin main \
    && git checkout -q -b behind HEAD~1 && echo "behind" >behind.txt && git add . && git commit -qm "Add behind" && git push -q origin behind ) >/dev/null 2>&1
"$GH" api user/repos -f name=target >/dev/null 2>&1

# ------------------------------------------------------------------------------------------------
# auth

K="auth login" t "auth login --with-token"   0 ""                            login_as "$TOKEN"
t "auth status (stored token)"               0 "account $ALICE"              gh2 auth status --hostname "$HOST"
K="auth login" t "auth login (second account)" 0 ""                          login_as "$BOB_TOKEN"
t "auth status (two accounts)"               0 "account $BOB"                gh2 auth status --hostname "$HOST"
t "auth switch"                              0 "$ALICE"                      gh2 auth switch --hostname "$HOST" --user "$ALICE"
t "auth token"                               0 "^$TOKEN\$"                   gh2 auth token --hostname "$HOST"
t "auth token --user"                        0 "^$BOB_TOKEN\$"               gh2 auth token --hostname "$HOST" --user "$BOB"
t "auth setup-git"                           0 ""                            gh2 auth setup-git --hostname "$HOST"
t "auth setup-git result"                    0 "auth git-credential"         git config --file "$WORK/gitconfig2" --get-all "credential.https://$HOST.helper"
K="auth refresh" t "auth refresh (device flow)" 0 "Authentication complete"   refresh_via_device_flow
K="auth refresh" t "auth refresh result"     0 "account $ALICE"              gh2 auth status --hostname "$HOST"
t "auth logout"                              0 "$BOB"                        gh2 auth logout --hostname "$HOST" --user "$BOB"
K="auth logout" t "auth logout result"       0 "account $ALICE"              gh2 auth status --hostname "$HOST"

# ------------------------------------------------------------------------------------------------
# browse (--no-browser prints the URL)

t "browse -n (repo)"                         0 "^https://$HOST/$ALICE/demo\$" "$GH" browse -n -R "$R"
t "browse -n (issue)"                        0 "^https://$HOST/$ALICE/demo/issues/1\$" "$GH" browse 1 -n -R "$R"
t "browse -n (file)"                         0 "^https://$HOST/$ALICE/demo/(tree|blob)/main/README.md\$" "$GH" browse README.md -n -R "$R"
t "browse -n --settings"                     0 "^https://$HOST/$ALICE/demo/settings\$" "$GH" browse -n -s -R "$R"
t "browse -n --releases"                     0 "^https://$HOST/$ALICE/demo/releases\$" "$GH" browse -n -r -R "$R"
t "browse -n --commit"                       0 "^https://$HOST/$ALICE/demo/(commit|tree)/[0-9a-f]{40}\$" "$GH" browse -n -c -R "$R"

# ------------------------------------------------------------------------------------------------
# repo: archive / rename / set-default / sync / deploy keys / autolinks / templates / read-*

t "repo archive"                             0 ""                            "$GH" repo archive "$HOST/$ALICE/readme" --yes
K="repo archive" t "repo archive result"     0 "\"isArchived\": *true"       "$GH" repo view "$HOST/$ALICE/readme" --json isArchived
t "repo unarchive"                           0 ""                            "$GH" repo unarchive "$HOST/$ALICE/readme" --yes
K="repo unarchive" t "repo unarchive result" 0 "\"isArchived\": *false"      "$GH" repo view "$HOST/$ALICE/readme" --json isArchived
t "repo rename"                              0 ""                            "$GH" repo rename renamed -R "$HOST/$ALICE/readme" --yes
K="repo rename" t "repo rename result"       0 "\"nameWithOwner\": *\"$ALICE/renamed\"" "$GH" repo view "$HOST/$ALICE/renamed" --json nameWithOwner
t "repo edit --add-topic"                    0 ""                            "$GH" repo edit "$R" --add-topic forge --add-topic demo
t "repo view --json (topics, more fields)"   0 "\"name\": *\"forge\""        "$GH" repo view "$R" --json repositoryTopics,languages,latestRelease,createdAt,pushedAt,sshUrl,isTemplate,mirrorUrl,licenseInfo,diskUsage
t "repo list --json --jq"                    0 "^$ALICE/demo\$"              "$GH" repo list "$ALICE" --json nameWithOwner --jq '.[].nameWithOwner'
t "repo list --topic"                        0 "$ALICE/demo"                 "$GH" repo list "$ALICE" --topic forge

CASE_DIR="$WORK/work/clone"
t "repo set-default"                         0 ""                            "$GH" repo set-default "$ALICE/demo"
t "repo set-default --view"                  0 "$ALICE/demo"                 "$GH" repo set-default --view
t "repo sync (local)"                        0 ""                            "$GH" repo sync --branch main
K="repo sync" t "repo sync (local) result"   0 "Add CI workflow"             git log -1 --format=%s origin/main
CASE_DIR="$WORK/work"
t "repo sync (fork from parent)"             0 ""                            as_bob "$GH" repo sync "$HOST/$BOB/demo" --source "$HOST/$ALICE/demo"
K="repo sync" t "repo sync (fork) result"    0 "Add CI workflow"             as_bob "$GH" api "repos/$BOB/demo/commits/main" --jq .commit.message

ssh-keygen -q -t ed25519 -N '' -C deploy -f "$WORK/deploy_key" >/dev/null 2>&1
ssh-keygen -q -t ed25519 -N '' -C user -f "$WORK/user_key" >/dev/null 2>&1
t "repo deploy-key add"                      0 ""                            "$GH" repo deploy-key add "$WORK/deploy_key.pub" -R "$R" --title "CI key"
t "repo deploy-key list"                     0 "CI key"                      "$GH" repo deploy-key list -R "$R"
t "repo deploy-key list --json"              0 "\"title\": *\"CI key\""      "$GH" repo deploy-key list -R "$R" --json id,title,key,readOnly,createdAt
DKID="$(api_q "repos/$ALICE/demo/keys" --jq '.[0].id')"
t "repo deploy-key delete"                   0 ""                            "$GH" repo deploy-key delete "$DKID" -R "$R"

t "repo autolink create"                     0 "Created repository autolink" "$GH" repo autolink create "TICKET-" "https://tracker.example.com/<num>" -R "$R"
t "repo autolink list"                       0 "TICKET-"                     "$GH" repo autolink list -R "$R"
t "repo autolink list --json"                0 "\"keyPrefix\": *\"TICKET-\"" "$GH" repo autolink list -R "$R" --json id,keyPrefix,urlTemplate,isAlphanumeric
ALID="$(api_q "repos/$ALICE/demo/autolinks" --jq '.[0].id')"
t "repo autolink view"                       0 "TICKET-"                     "$GH" repo autolink view "$ALID" -R "$R"
t "repo autolink delete"                     0 ""                            "$GH" repo autolink delete "$ALID" -R "$R" --yes

t "repo gitignore list"                      0 "Rust"                        "$GH" repo gitignore list
t "repo gitignore view"                      0 "target"                      "$GH" repo gitignore view Rust
t "repo license list"                        0 "MIT"                         "$GH" repo license list
t "repo license view"                        0 "MIT License"                 "$GH" repo license view mit

t "repo read-file"                           0 "# demo"                      "$GH" repo read-file README.md -R "$R"
t "repo read-file --ref --json"              0 "feature4"                    "$GH" repo read-file feature4.txt -R "$R" --ref feature4 --json path,content
t "repo read-dir"                            0 "README.md"                   "$GH" repo read-dir -R "$R"
t "repo read-dir PATH --json"                0 "guide.md"                    "$GH" repo read-dir docs -R "$R" --json name,path,type

# ------------------------------------------------------------------------------------------------
# issues: transfer / pin / lock / develop / more --json

t "issue create (for transfer)"              0 "/issues/"                    "$GH" issue create -R "$R" -t "Move me" -b "transfer" -l bug
TNUM="$(last_num)"
t "issue transfer"                           0 "https://$HOST/$ALICE/target/issues/[0-9]+" "$GH" issue transfer "$TNUM" "$HOST/$ALICE/target" -R "$R"
K="issue transfer" t "issue transfer result" 0 "Move me"                     "$GH" issue list -R "$HOST/$ALICE/target" --json title,number
K="issue transfer" t "issue transfer (gone from source)" 1 "Could not resolve|not found" "$GH" issue view "$TNUM" -R "$R"
t "issue pin"                                0 "Pinned issue"                "$GH" issue pin 1 -R "$R"
K="issue pin" t "issue pin result"           0 "\"isPinned\": *true"         "$GH" issue view 1 -R "$R" --json isPinned
t "issue unpin"                              0 "Unpinned issue"              "$GH" issue unpin 1 -R "$R"
K="issue unpin" t "issue unpin result"       0 "\"isPinned\": *false"        "$GH" issue view 1 -R "$R" --json isPinned
# (lock/unlock print their confirmation only on a terminal)
t "issue lock"                               0 ""                            "$GH" issue lock 1 -R "$R" --reason resolved
K="issue lock" t "issue lock result"         0 "^true resolved\$"            "$GH" api "repos/$ALICE/demo/issues/1" --jq '"\(.locked) \(.active_lock_reason)"'
t "issue unlock"                             0 ""                            "$GH" issue unlock 1 -R "$R"
K="issue unlock" t "issue unlock result"     0 "^false\$"                    "$GH" api "repos/$ALICE/demo/issues/1" --jq '.locked'
t "issue develop"                            0 "$ALICE/demo/tree/1-first-bug" "$GH" issue develop 1 -R "$R" --name 1-first-bug --base main
t "issue develop --list"                     0 "1-first-bug"                 "$GH" issue develop 1 -R "$R" --list
K="issue develop" t "issue develop result"   0 "1-first-bug"                 "$GH" api "repos/$ALICE/demo/branches" --jq '.[].name'
t "issue comment --edit-last"                0 "#issuecomment-"              "$GH" issue comment 1 -R "$R" --edit-last -b "An edited comment"
t "issue status --json"                      0 "\"createdBy\""               "$GH" issue status -R "$R" --json number,title
t "issue list --json --jq"                   0 "^First bug"                  "$GH" issue list -R "$R" --json title --jq '.[].title'
"$GH" api "repos/$ALICE/demo/milestones" -f title="v1 milestone" >/dev/null 2>&1
t "issue create --milestone"                 0 "/issues/"                    "$GH" issue create -R "$R" -t "Milestoned" -b "m" --milestone "v1 milestone"
MNUM="$(last_num)"
t "issue list --milestone"                   0 "Milestoned"                  "$GH" issue list -R "$R" --milestone "v1 milestone"
t "issue edit (two issues)"                  0 ""                            "$GH" issue edit 1 "$MNUM" -R "$R" --add-label enhancement
K="issue close" t "issue close --json view"                  0 "\"stateReason\": *\"COMPLETED\"" sh -c "'$GH' issue close 1 -R '$R' >/dev/null && '$GH' issue view 1 -R '$R' --json state,stateReason,closedAt"
K="issue reopen" t "issue reopen (again)"    0 "Reopened"                    "$GH" issue reopen 1 -R "$R"

# ------------------------------------------------------------------------------------------------
# pull requests: lock / revert / update-branch / more variants

t "pr lock"                                  0 ""                            "$GH" pr lock 5 -R "$R" --reason off_topic
K="pr lock" t "pr lock result"               0 "^true off-topic\$"           "$GH" api "repos/$ALICE/demo/issues/5" --jq '"\(.locked) \(.active_lock_reason)"'
t "pr unlock"                                0 ""                            "$GH" pr unlock 5 -R "$R"
K="pr unlock" t "pr unlock result"           0 "^false\$"                    "$GH" api "repos/$ALICE/demo/issues/5" --jq .locked
t "pr update-branch"                         0 ""                            "$GH" pr update-branch 5 -R "$R"
K="pr update-branch" t "pr update-branch result" 0 "Merge branch 'main'"     "$GH" api "repos/$ALICE/demo/commits/feature2" --jq .commit.message
t "pr create (behind base)"                  0 "/pull/"                      "$GH" pr create -R "$R" --head behind --base main -t "Behind PR" -b "behind"
BNUM="$(last_num)"
t "pr update-branch --rebase"                0 ""                            "$GH" pr update-branch "$BNUM" -R "$R" --rebase
K="pr update-branch" t "pr update-branch --rebase result" 0 "Add CI workflow" "$GH" api "repos/$ALICE/demo/commits/behind~1" --jq .commit.message
t "pr revert"                                0 "/pull/[0-9]+"                "$GH" pr revert 4 -R "$R"
RNUM="$(last_num)"
K="pr revert" t "pr revert result"           0 "Revert \\\\?\"Add feature"    "$GH" pr view "$RNUM" -R "$R" --json title,body,headRefName
K="pr revert" t "pr revert diff"             0 "^-feature change"            "$GH" pr diff "$RNUM" -R "$R"
t "pr comment --edit-last"                   0 "#issuecomment-"              "$GH" pr comment 5 -R "$R" --edit-last -b "Edited PR comment"
t "pr view --comments"                       0 "Edited PR comment"           "$GH" pr view 5 -R "$R" --comments
t "pr status --json"                         0 "\"createdBy\""               "$GH" pr status -R "$R" --json number,title,state
t "pr checks --json"                         0 "\"name\": *\"ci/test\""      "$GH" pr checks 4 -R "$R" --json name,state,bucket
t "pr diff --patch"                          0 "^Subject: \\[PATCH"          "$GH" pr diff 5 -R "$R" --patch
t "pr list --json --jq"                      0 "Add feature2"                "$GH" pr list -R "$R" --json title --jq '.[].title'
t "pr list --label"                          0 "Add feature2"                "$GH" pr list -R "$R" --label bug
t "pr edit --add-reviewer"                   0 "/pull/5"                     "$GH" pr edit 5 -R "$R" --add-reviewer "$BOB" --body "New body"
t "pr review --request-changes -> approve"   0 ""                            as_bob "$GH" pr review 5 -R "$R" --approve -b "Fixed"

# ------------------------------------------------------------------------------------------------
# labels

t "label edit"                               0 ""                            "$GH" label edit triage --name triaged --color 123456 --description "Triaged" -R "$R"
K="label edit" t "label edit result"         0 "\"name\": *\"triaged\""      "$GH" label list -R "$R" --json name,color,description
t "label clone"                              0 ""                            "$GH" label clone "$HOST/$ALICE/demo" -R "$HOST/$ALICE/target" --force
K="label clone" t "label clone result"       0 "triaged"                     "$GH" label list -R "$HOST/$ALICE/target"
t "label delete"                             0 ""                            "$GH" label delete gql-label -R "$R" --yes
K="label delete" t "label delete result"     1 "Not Found"                   "$GH" api "repos/$ALICE/demo/labels/gql-label"

# ------------------------------------------------------------------------------------------------
# releases

echo "asset content" >"$WORK/asset.txt"
head -c 2048 /dev/urandom >"$WORK/asset2.bin"
t "release create (with asset)"              0 "/releases/tag/v1.1"          "$GH" release create v1.1 "$WORK/asset.txt" -R "$R" --title "v1.1" --notes "Second release" --target main
t "release create --draft --generate-notes"  0 "/releases/"                  "$GH" release create v2.0-rc -R "$R" --draft --generate-notes --target main
t "release upload"                           0 ""                            "$GH" release upload v1.1 "$WORK/asset2.bin" -R "$R"
t "release view --json"                      0 "asset2.bin"                  "$GH" release view v1.1 -R "$R" --json tagName,name,assets,body,isDraft,isPrerelease,url,author,createdAt,publishedAt,tarballUrl,zipballUrl
t "release list --json"                      0 "\"tagName\": *\"v1.1\""      "$GH" release list -R "$R" --json tagName,name,isLatest,isDraft,isPrerelease,publishedAt
t "release download (pattern)"               0 ""                            "$GH" release download v1.1 -R "$R" -D "$WORK/dl" -p 'asset*'
K="release download" t "release download result" 0 "asset content"          cat "$WORK/dl/asset.txt"
t "release download --archive"               0 ""                            "$GH" release download v1.1 -R "$R" -D "$WORK/dl" --archive tar.gz
K="release download" t "release download --archive result" 0 "README.md"    tar -tzf "$WORK/dl/demo-v1.1.tar.gz"
t "release edit"                             0 ""                            "$GH" release edit v1.1 -R "$R" --title "v1.1 final" --prerelease
K="release edit" t "release edit result"     0 "\"isPrerelease\": *true"     "$GH" release view v1.1 -R "$R" --json name,isPrerelease
t "release delete-asset"                     0 ""                            "$GH" release delete-asset v1.1 asset2.bin -R "$R" --yes
K="release delete-asset" t "release delete-asset result" 0 "^1\$"          "$GH" release view v1.1 -R "$R" --json assets --jq '.assets | length'
t "release delete"                           0 ""                            "$GH" release delete v1.1 -R "$R" --yes --cleanup-tag
K="release delete" t "release delete result" 1 "release not found"           "$GH" release view v1.1 -R "$R"

# ------------------------------------------------------------------------------------------------
# gists

echo "hello gist" >"$WORK/hello.txt"
echo "second file" >"$WORK/second.txt"
echo "hello again" >"$WORK/hello2.txt"
t "gist create"                              0 "https://$HOST/gist/"         "$GH" gist create "$WORK/hello.txt" -d "My gist" --public
GIST_ID="$(grep -Eo '[0-9a-f]{20,}' "$(last_log)" | tail -1)"
t "gist create (stdin)"                      0 "https://$HOST/gist/"         sh -c "echo piped | '$GH' gist create -f piped.txt -"
t "gist list"                                0 "My gist"                     "$GH" gist list
t "gist list --public"                       0 "My gist"                     "$GH" gist list --public
t "gist view"                                0 "hello gist"                  "$GH" gist view "$GIST_ID" --raw
t "gist view --files"                        0 "hello.txt"                   "$GH" gist view "$GIST_ID" --files
t "gist edit --add"                          0 ""                            "$GH" gist edit "$GIST_ID" -a "$WORK/second.txt"
t "gist edit FILE (replace content)"         0 ""                            "$GH" gist edit "$GIST_ID" -f hello.txt "$WORK/hello2.txt"
K="gist edit" t "gist edit result"           0 "hello again"                 "$GH" gist view "$GIST_ID" --raw -f hello.txt
t "gist edit --desc"                         0 ""                            "$GH" gist edit "$GIST_ID" -f hello.txt -d "Renamed gist"
t "gist rename"                              0 ""                            "$GH" gist rename "$GIST_ID" hello.txt greeting.txt
K="gist rename" t "gist rename result"       0 "greeting.txt"                "$GH" gist view "$GIST_ID" --files
t "gist clone"                               0 ""                            "$GH" gist clone "$GIST_ID" "$WORK/work/gistclone"
K="gist clone" t "gist clone result"         0 "greeting.txt"                ls "$WORK/work/gistclone"
t "gist delete"                              0 ""                            "$GH" gist delete "$GIST_ID" --yes
K="gist delete" t "gist delete result"       1 "404|not found"               "$GH" gist view "$GIST_ID"

# ------------------------------------------------------------------------------------------------
# ssh / gpg keys

t "ssh-key add"                              0 ""                            "$GH" ssh-key add "$WORK/user_key.pub" --title "laptop"
t "ssh-key add --type signing"               0 ""                            "$GH" ssh-key add "$WORK/user_key.pub" --title "signing" --type signing
t "ssh-key list"                             0 "laptop"                      "$GH" ssh-key list
SKID="$(api_q user/keys --jq '.[] | select(.title=="laptop") | .id')"
t "ssh-key delete"                           0 ""                            "$GH" ssh-key delete "$SKID" --yes
K="ssh-key delete" t "ssh-key delete result" 0 "^0\$"                        "$GH" api user/keys --jq length

cat >"$WORK/gpg.mjs" <<'EOF'
// Emits an ASCII-armored OpenPGP v4 ed25519 public key (key + user id packets).
import crypto from 'node:crypto';
const { publicKey } = crypto.generateKeyPairSync('ed25519');
const raw = publicKey.export({ format: 'der', type: 'spki' }).subarray(-32);
const oid = Buffer.from([0x2b, 0x06, 0x01, 0x04, 0x01, 0xda, 0x47, 0x0f, 0x01]);
const mpi = Buffer.concat([Buffer.from([0x40]), raw]);
const now = Math.floor(Date.now() / 1000) - 60;
const body = Buffer.concat([Buffer.from([4, (now >>> 24) & 255, (now >>> 16) & 255, (now >>> 8) & 255, now & 255, 22, oid.length]), oid,
  Buffer.from([((mpi.length * 8 - 1) >> 8) & 255, (mpi.length * 8 - 1) & 255]), mpi]);
const pkt = (tag, b) => Buffer.concat([Buffer.from([0xc0 | tag, b.length]), b]);
const all = Buffer.concat([pkt(6, body), pkt(13, Buffer.from(process.argv[2] || 'Test <test@example.com>'))]);
let crc = 0xb704ce;
for (const byte of all) { crc ^= byte << 16; for (let i = 0; i < 8; i++) { crc <<= 1; if (crc & 0x1000000) crc ^= 0x1864cfb; } }
const c = Buffer.from([(crc >> 16) & 255, (crc >> 8) & 255, crc & 255]).toString('base64');
console.log('-----BEGIN PGP PUBLIC KEY BLOCK-----\n\n' + all.toString('base64').match(/.{1,64}/g).join('\n') + '\n=' + c + '\n-----END PGP PUBLIC KEY BLOCK-----');
EOF
node "$WORK/gpg.mjs" "Alice <$ALICE@example.com>" >"$WORK/key.asc" 2>/dev/null
t "gpg-key add"                              0 ""                            "$GH" gpg-key add "$WORK/key.asc" --title "work key"
t "gpg-key list"                             0 "$ALICE@example.com"          "$GH" gpg-key list
GPGID="$(api_q user/gpg_keys --jq '.[0].key_id')"
t "gpg-key delete"                           0 ""                            "$GH" gpg-key delete "$GPGID" --yes
K="gpg-key delete" t "gpg-key delete result" 0 "^0\$"                        "$GH" api user/gpg_keys --jq length

# ------------------------------------------------------------------------------------------------
# org, search, status, api extras

ORG="acme-$SUFFIX"
"$FORGE" admin create-org --login "$ORG" --owner "$ALICE" >/dev/null 2>&1 \
  || "$GH" api admin/organizations -f login="$ORG" -f admin="$ALICE" >/dev/null 2>&1
t "org list"                                 0 "$ORG"                        "$GH" org list

t "search repos"                             0 "$ALICE/demo"                 "$GH" search repos demo --owner "$ALICE"
t "search repos --json"                      0 "\"fullName\": *\"$ALICE/demo\"" "$GH" search repos demo --owner "$ALICE" --json fullName,description,stargazersCount,visibility,url
t "search issues"                            0 "First bug"                   "$GH" search issues First --repo "$ALICE/demo"
t "search issues --json"                     0 "\"title\": *\"First bug"     "$GH" search issues First --repo "$ALICE/demo" --json number,title,state,repository,author,labels,url
t "search prs"                               0 "Add feature2"                "$GH" search prs feature2 --repo "$ALICE/demo"
t "search prs --json --state"                0 "\"title\": *\"Add feature2\"" "$GH" search prs feature2 --repo "$ALICE/demo" --state open --json number,title,state,isDraft,repository
t "search commits"                           0 "Add feature"                 "$GH" search commits feature --repo "$ALICE/demo"
t "search commits --json"                    0 "\"sha\""                     "$GH" search commits feature --repo "$ALICE/demo" --json sha,commit,repository,url
t "search code"                              0 "README.md"                   "$GH" search code demo --repo "$ALICE/demo"
t "search code --json"                       0 "\"path\": *\"README.md\""    "$GH" search code demo --repo "$ALICE/demo" --json path,repository,sha,url,textMatches

t "status"                                   0 "Assigned Issues"             "$GH" status
t "status --org"                             0 "Review Requests"             "$GH" status -e "$ALICE/target"

t "api -X PATCH"                             0 "^patched\$"                  "$GH" api -X PATCH "repos/$ALICE/demo" -f description=patched --jq .description
t "api --include"                            0 "X-Github-Request-Id"         "$GH" api -i user
t "api --template"                           0 "^$ALICE\$"                   "$GH" api user --template '{{.login}}'
t "api --input"                              0 "\"name\": *\"from-input\""   sh -c "echo '{\"name\":\"from-input\",\"color\":\"ededed\"}' | '$GH' api repos/$ALICE/demo/labels --input -"
t "api graphql -F variables"                 0 "\"name\": *\"demo\""         "$GH" api graphql -F owner="$ALICE" -F name=demo -f query='query($owner: String!, $name: String!) { repository(owner: $owner, name: $name) { name } }'
t "api 404"                                  1 "Not Found"                   "$GH" api repos/nobody/nothing

# ------------------------------------------------------------------------------------------------
# rulesets

"$GH" api -X POST "repos/$ALICE/demo/rulesets" --input - >/dev/null 2>&1 <<EOF
{"name": "protect-main", "target": "branch", "enforcement": "active",
 "conditions": {"ref_name": {"include": ["~DEFAULT_BRANCH"], "exclude": []}},
 "rules": [{"type": "deletion"}, {"type": "non_fast_forward"}]}
EOF
t "ruleset list"                             0 "protect-main"                "$GH" ruleset list -R "$R"
RSID="$(api_q "repos/$ALICE/demo/rulesets" --jq '.[0].id')"
t "ruleset view"                             0 "protect-main"                "$GH" ruleset view "$RSID" -R "$R"
t "ruleset check"                            0 "deletion"                    "$GH" ruleset check main -R "$R"

# ------------------------------------------------------------------------------------------------
# Actions: workflows, runs, caches, secrets, variables

t "workflow list"                            0 "CI"                          "$GH" workflow list -R "$R"
t "workflow list --json"                     0 "\"path\": *\".github/workflows/ci.yml\"" "$GH" workflow list -R "$R" --json id,name,path,state
t "workflow view"                            0 "CI"                          "$GH" workflow view ci.yml -R "$R"
t "workflow disable"                         0 ""                            "$GH" workflow disable ci.yml -R "$R"
K="workflow disable" t "workflow disable result" 0 "disabled_manually"       "$GH" workflow list -R "$R" --all --json state
t "workflow enable"                          0 ""                            "$GH" workflow enable ci.yml -R "$R"
K="workflow enable" t "workflow enable result" 0 "\"state\": *\"active\""    "$GH" workflow list -R "$R" --json state
t "workflow run"                             0 ""                            "$GH" workflow run ci.yml -R "$R" --ref main
t "run list"                                 0 "CI"                          "$GH" run list -R "$R"
t "run list --json"                          0 "\"workflowName\": *\"CI\""   "$GH" run list -R "$R" --json databaseId,name,workflowName,status,conclusion,headBranch,event,createdAt
RUN_ID="$(api_q "repos/$ALICE/demo/actions/runs" --jq '.workflow_runs[0].id')"
t "run view"                                 0 "CI"                          "$GH" run view "$RUN_ID" -R "$R"
t "run view --json"                          0 "\"status\""                  "$GH" run view "$RUN_ID" -R "$R" --json databaseId,status,conclusion,jobs,workflowName
t "run cancel"                               0 ""                            "$GH" run cancel "$RUN_ID" -R "$R"
t "run watch"                                0 ""                            "$GH" run watch "$RUN_ID" -R "$R"
t "run rerun"                                0 ""                            "$GH" run rerun "$RUN_ID" -R "$R"
t "run download"                             1 "no valid artifacts"          "$GH" run download "$RUN_ID" -R "$R" -D "$WORK/run-dl"
t "run delete"                               0 ""                            "$GH" run delete "$RUN_ID" -R "$R"
t "cache list"                               0 ""                            "$GH" cache list -R "$R"
t "cache list --json"                        0 "^\\[\\]\$"                   "$GH" cache list -R "$R" --json id,key
t "cache delete --all"                       0 ""                            "$GH" cache delete --all --succeed-on-no-caches -R "$R"

t "secret set"                               0 ""                            "$GH" secret set API_KEY -b "s3cret" -R "$R"
t "secret list"                              0 "API_KEY"                     "$GH" secret list -R "$R"
t "secret list --json"                       0 "\"name\": *\"API_KEY\""      "$GH" secret list -R "$R" --json name,updatedAt
t "secret delete"                            0 ""                            "$GH" secret delete API_KEY -R "$R"
K="secret delete" t "secret delete result"   0 "^\\[\\]\$"                   "$GH" secret list -R "$R" --json name
t "variable set"                             0 ""                            "$GH" variable set GREETING -b hello -R "$R"
t "variable list"                            0 "GREETING"                    "$GH" variable list -R "$R"
t "variable list --json"                     0 "\"value\": *\"hello\""       "$GH" variable list -R "$R" --json name,value,updatedAt
t "variable get"                             0 "^hello\$"                    "$GH" variable get GREETING -R "$R"
t "variable set (update)"                    0 ""                            "$GH" variable set GREETING -b howdy -R "$R"
K="variable set" t "variable set result"     0 "^howdy\$"                    "$GH" variable get GREETING -R "$R"
t "variable delete"                          0 ""                            "$GH" variable delete GREETING -R "$R"

# ------------------------------------------------------------------------------------------------
# Actions with a runner (forge only): a `forge runner` registered to the repository runs the
# jobs, so runs complete for real (watch, logs, artifacts, re-run of failed jobs, secrets).

if [ "${GH_SUITE_EXTERNAL:-}" != "1" ]; then
  REG_TOKEN="$(api_q -X POST "repos/$ALICE/demo/actions/runners/registration-token" --jq .token)"
  mkdir -p "$WORK/runner"
  (exec -a "forge-gh-suite-runner-$PORT" "$FORGE" runner --url "http://127.0.0.1:$PORT" --token "$REG_TOKEN" --name gh-suite \
    --work-dir "$WORK/runner" --slots 2 >"$WORK/runner.log" 2>&1) &
  RUNNER_PID=$!
  BUILD_WF="name: Build
on: workflow_dispatch
jobs:
  build:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - run: echo \"secret=\${{ secrets.BUILD_TOKEN }}\" && echo build-ok && ls README.md
      - run: mkdir -p out && echo payload > out/result.txt
      - uses: actions/upload-artifact@v4
        with:
          name: result
          path: out/
  flaky:
    runs-on: ubuntu-latest
    steps:
      - run: if [ \"\${{ github.run_attempt }}\" = 1 ]; then echo failing-first-attempt; exit 1; fi; echo second-attempt-ok
"
  api_q -X PUT "repos/$ALICE/demo/contents/.github/workflows/build.yml" -f message="Add build workflow" \
    -f content="$(printf '%s' "$BUILD_WF" | base64 | tr -d '\n')" >/dev/null
  K="secret set" t "secret set (for a job)"      0 ""                          "$GH" secret set BUILD_TOKEN -b "t0p-s3cret" -R "$R"
  K="workflow run" t "workflow run (runner)"     0 ""                          "$GH" workflow run build.yml -R "$R" --ref main
  for _ in $(seq 1 50); do
    BUILD_RUN="$(api_q "repos/$ALICE/demo/actions/workflows/build.yml/runs" --jq '.workflow_runs[0].id')"
    [ -n "$BUILD_RUN" ] && break; sleep 0.2
  done
  K="run watch" t "run watch --exit-status (failure)" 1 "flaky"               "$GH" run watch "$BUILD_RUN" -R "$R" --exit-status -i 1
  K="run view" t "run view (completed jobs)"     0 "X main Build"                 "$GH" run view "$BUILD_RUN" -R "$R"
  K="run view" t "run view --log"                0 "build-ok"                  "$GH" run view "$BUILD_RUN" -R "$R" --log
  K="run view" t "run view --log (secret masked)" 0 "secret=\\*\\*\\*"         "$GH" run view "$BUILD_RUN" -R "$R" --log
  K="run view" t "run view --log-failed"         0 "failing-first-attempt"     "$GH" run view "$BUILD_RUN" -R "$R" --log-failed
  K="run download" t "run download (artifact)"   0 ""                          "$GH" run download "$BUILD_RUN" -R "$R" -n result -D "$WORK/run-dl2"
  K="run download" t "run download result"       0 "^payload\$"                cat "$WORK/run-dl2/result.txt"
  K="run rerun" t "run rerun --failed"           0 ""                          "$GH" run rerun "$BUILD_RUN" -R "$R" --failed
  K="run watch" t "run watch --exit-status (rerun succeeds)" 0 ""             "$GH" run watch "$BUILD_RUN" -R "$R" --exit-status -i 1
  K="run list" t "run list --json (completed)"   0 "\"conclusion\": *\"success\"" "$GH" run list -R "$R" --workflow build.yml --json conclusion,status --limit 1
  K="run view" t "run view --json jobs (attempt 2)" 0 "second-attempt|\"attempt\": *2" "$GH" run view "$BUILD_RUN" -R "$R" --json attempt,jobs
  K="secret delete" t "secret delete (job secret)" 0 ""                        "$GH" secret delete BUILD_TOKEN -R "$R"
  kill "$RUNNER_PID" 2>/dev/null; pkill -f "forge-gh-suite-runner-$PORT" 2>/dev/null
fi

# ------------------------------------------------------------------------------------------------
# local-only commands: alias, config, completion, licenses

t "alias set"                                0 ""                            "$GH" alias set iv 'issue view'
K="alias set" t "alias set (use)"            0 "First bug"                   "$GH" iv 1 -R "$R"
t "alias list"                               0 "iv"                          "$GH" alias list
printf 'il: issue list\n' >"$WORK/aliases.yml"
t "alias import"                             0 ""                            "$GH" alias import "$WORK/aliases.yml"
K="alias import" t "alias import (use)"      0 "First bug"                   "$GH" il -R "$R"
t "alias delete"                             0 ""                            "$GH" alias delete iv
t "config set"                               0 ""                            "$GH" config set editor vim
t "config get"                               0 "^vim\$"                      "$GH" config get editor
t "config list"                              0 "editor=vim"                  "$GH" config list
t "config clear-cache"                       0 ""                            "$GH" config clear-cache
t "completion"                               0 "complete"                    "$GH" completion -s bash
t "licenses"                                 0 "License"                     "$GH" licenses

# ------------------------------------------------------------------------------------------------
# extensions (installed from a forge repository) and agent skills

EXT="$WORK/ext-src"
"$GH" api user/repos -f name=gh-hello >/dev/null 2>&1
git_q clone "https://$HOST/$ALICE/gh-hello.git" "$EXT"
( cd "$EXT" && printf '#!/bin/sh\necho "hello from forge"\n' >gh-hello && chmod +x gh-hello && git add . \
    && git commit -qm "hello extension" && git push -q origin HEAD:main ) >/dev/null 2>&1
"$GH" api -X PUT "repos/$ALICE/gh-hello/topics" -f 'names[]=gh-extension' >/dev/null 2>&1
t "extension install"                        0 ""                            "$GH" extension install "$HOST/$ALICE/gh-hello"
t "extension list"                           0 "hello"                       "$GH" extension list
t "extension exec"                           0 "hello from forge"            "$GH" extension exec hello
( cd "$EXT" && printf '#!/bin/sh\necho "hello again from forge"\n' >gh-hello && git commit -qam "v2" && git push -q origin HEAD:main ) >/dev/null 2>&1
t "extension upgrade"                        0 ""                            "$GH" extension upgrade hello
K="extension upgrade" t "extension upgrade result" 0 "hello again"           "$GH" hello
t "extension search"                         0 "gh-hello"                    "$GH" extension search hello
t "extension remove"                         0 ""                            "$GH" extension remove hello
t "extension create"                         0 ""                            "$GH" extension create scaffold
K="extension create" t "extension create result" 0 "gh-scaffold"            ls "$WORK/work/gh-scaffold"

# Agent skills: every skill command except `list` refuses non-github.com hosts inside gh
# (category b); `skill list` only scans a local directory.
mkdir -p "$WORK/installed-skills/hello-skill"
printf -- '---\nname: hello-skill\ndescription: Says hello from forge\n---\n\n# Hello skill\n' >"$WORK/installed-skills/hello-skill/SKILL.md"
t "skill list"                               0 "hello-skill"                 "$GH" skill list --dir "$WORK/installed-skills"

# ------------------------------------------------------------------------------------------------
# discussions (numbers come from the repository's issue/PR sequence, like GitHub)

t "repo edit --enable-discussions"           0 ""                            "$GH" repo edit "$R" --enable-discussions
t "discussion create"                        0 "/discussions/[0-9]+"         "$GH" discussion create -R "$R" --title "Hello forge" --body "Let us discuss" --category General
DNUM="$(grep -Eo '/discussions/[0-9]+' "$(last_log)" | grep -Eo '[0-9]+$')"
t "discussion create (Q&A)"                  0 "/discussions/[0-9]+"         "$GH" discussion create -R "$R" --title "How do I?" --body "Question" --category "Q&A"
t "discussion list"                          0 "Hello forge"                 "$GH" discussion list -R "$R"
t "discussion list --json"                   0 "\"title\": *\"Hello forge\"" "$GH" discussion list -R "$R" --json number,title,category,author
t "discussion list --category"               0 "How do I"                    "$GH" discussion list -R "$R" --category "Q&A"
t "discussion view"                          0 "Let us discuss"              "$GH" discussion view "$DNUM" -R "$R"
t "discussion comment"                       0 "discussioncomment-"          "$GH" discussion comment "$DNUM" -R "$R" --body "A reply"
t "discussion view --comments"               0 "A reply"                     "$GH" discussion view "$DNUM" -R "$R" --comments
t "discussion edit"                          0 ""                            "$GH" discussion edit "$DNUM" -R "$R" --title "Hello forge (edited)" --category Ideas
K="discussion edit" t "discussion edit result" 0 "\"name\": *\"Ideas\""     "$GH" discussion view "$DNUM" -R "$R" --json title,category

# ------------------------------------------------------------------------------------------------
# projects (v2) — without a terminal gh prints nothing on success, so cases use --format json or
# check the result with a follow-up query.

pj() { "$GH" api graphql -f query="$1" "${@:2}"; }
t "project create"                           0 "\"title\":\"Roadmap\""       "$GH" project create --owner "$ALICE" --title "Roadmap" --format json
t "project list"                             0 "Roadmap"                     "$GH" project list --owner "$ALICE"
t "project list --format json"               0 "\"number\":1,"               "$GH" project list --owner "$ALICE" --format json
t "project view"                             0 "Roadmap"                     "$GH" project view 1 --owner "$ALICE"
t "project edit"                             0 "\"shortDescription\":\"Plans\"" "$GH" project edit 1 --owner "$ALICE" --description "Plans" --format json
t "project field-create"                     0 "\"name\":\"Priority\""       "$GH" project field-create 1 --owner "$ALICE" --name Priority --data-type SINGLE_SELECT --single-select-options "High,Low" --format json
t "project field-create (text)"              0 "\"name\":\"Notes\""          "$GH" project field-create 1 --owner "$ALICE" --name Notes --data-type TEXT --format json
t "project field-list"                       0 "Priority.*ProjectV2SingleSelectField" "$GH" project field-list 1 --owner "$ALICE"
FIELD_ID="$("$GH" project field-list 1 --owner "$ALICE" --format json --jq '.fields[] | select(.name=="Priority") | .id' 2>/dev/null)"
t "project item-create"                      0 "\"type\":\"DraftIssue\""     "$GH" project item-create 1 --owner "$ALICE" --title "Draft item" --body "todo" --format json
t "project item-add"                         0 "\"type\":\"Issue\""          "$GH" project item-add 1 --owner "$ALICE" --url "https://$HOST/$ALICE/demo/issues/1" --format json
t "project item-list"                        0 "Draft item"                  "$GH" project item-list 1 --owner "$ALICE"
ITEM_ID="$("$GH" project item-list 1 --owner "$ALICE" --format json --jq '.items[] | select(.content.title=="Draft item") | .id' 2>/dev/null)"
DRAFT_ID="$("$GH" project item-list 1 --owner "$ALICE" --format json --jq '.items[] | select(.content.title=="Draft item") | .content.id' 2>/dev/null)"
t "project item-edit (draft)"                0 "Draft item \\(edited\\)"     "$GH" project item-edit --id "$DRAFT_ID" --title "Draft item (edited)" --format json
t "project item-edit (field by name)"        0 ""                            "$GH" project item-edit 1 --owner "$ALICE" --url "https://$HOST/$ALICE/demo/issues/1" --field Priority --value High
K="project item-edit" t "project item-edit result" 0 "^High\$"               "$GH" project item-list 1 --owner "$ALICE" --format json --jq '.items[] | select(.content.number==1) | .priority'
t "project item-list --query"                0 "^1\$"                        "$GH" project item-list 1 --owner "$ALICE" --query 'priority:"High"' --format json --jq '.items | length'
t "project item-archive"                     0 ""                            "$GH" project item-archive 1 --owner "$ALICE" --id "$ITEM_ID"
K="project item-archive" t "project item-archive result" 0 "\"isArchived\": *true" pj 'query($id:ID!){node(id:$id){... on ProjectV2Item{isArchived}}}' -f id="$ITEM_ID"
t "project item-delete"                      0 ""                            "$GH" project item-delete 1 --owner "$ALICE" --id "$ITEM_ID"
K="project item-delete" t "project item-delete result" 0 "^1\$"              "$GH" project item-list 1 --owner "$ALICE" --format json --jq '.items | length'
t "project field-delete"                     0 ""                            "$GH" project field-delete --id "$FIELD_ID"
K="project field-delete" t "project field-delete result" 1 ""                sh -c "'$GH' project field-list 1 --owner '$ALICE' | grep -q Priority"
t "project link"                             0 ""                            "$GH" project link 1 --owner "$ALICE" --repo "$ALICE/demo"
K="project link" t "project link result"     0 "Roadmap"                     pj 'query($o:String!){repository(owner:$o,name:"demo"){projectsV2(first:5){nodes{title}}}}' -f o="$ALICE"
t "project unlink"                           0 ""                            "$GH" project unlink 1 --owner "$ALICE" --repo "$ALICE/demo"
K="project unlink" t "project unlink result" 0 "\"nodes\": *\\[\\]"          pj 'query($o:String!){repository(owner:$o,name:"demo"){projectsV2(first:5){nodes{title}}}}' -f o="$ALICE"
t "project copy"                             0 "\"title\":\"Roadmap copy\""  "$GH" project copy 1 --source-owner "$ALICE" --target-owner "$ALICE" --title "Roadmap copy" --format json
"$GH" project create --owner "$ORG" --title "Org board" >/dev/null 2>&1
t "project mark-template"                    0 ""                            "$GH" project mark-template 1 --owner "$ORG"
K="project mark-template" t "project mark-template result" 0 "\"template\": *true" pj 'query($o:String!){organization(login:$o){projectV2(number:1){template}}}' -f o="$ORG"
t "project close"                            0 "\"closed\":true"             "$GH" project close 1 --owner "$ALICE" --format json
t "project close --undo"                     0 "\"closed\":false"            "$GH" project close 1 --owner "$ALICE" --undo --format json
t "project delete"                           0 "\"title\":\"Roadmap copy\""  "$GH" project delete 2 --owner "$ALICE" --format json
K="project delete" t "project delete result" 1 "Could not resolve to a ProjectV2" "$GH" project view 2 --owner "$ALICE"


# ------------------------------------------------------------------------------------------------
# Report: per case, then per gh command against the catalogue (tests/gh/commands.tsv).
# A command "works" when it has at least one case and all of its cases pass.

PCT=$(( TOTAL > 0 ? PASS * 100 / TOTAL : 0 ))
REPORT="${GH_SUITE_REPORT:-$ROOT/tests/gh/report.md}"
CATALOGUE="$ROOT/tests/gh/commands.tsv"
CASES_TSV="$WORK/cases.tsv"
: >"$CASES_TSV"
for i in "${!NAMES[@]}"; do printf '%s\t%s\n' "${KEYS[$i]}" "${RESULTS[$i]}" >>"$CASES_TSV"; done
# command <TAB> category <TAB> status (PASS / FAIL / UNTESTED / n/a) <TAB> passing/total cases <TAB> note
CMD_TSV="$WORK/commands-result.tsv"
awk -F'\t' 'NR == FNR { n[$1]++; if ($2 == "PASS") p[$1]++; next }
  /^#/ || NF < 2 { next }
  { c = $1; st = ($2 != "a") ? "n/a" : (n[c] == 0 ? "UNTESTED" : (p[c] == n[c] ? "PASS" : "FAIL"))
    printf "%s\t%s\t%s\t%d/%d\t%s\n", c, $2, st, p[c], n[c], $3 }' "$CASES_TSV" "$CATALOGUE" >"$CMD_TSV"
UNKNOWN="$(awk -F'\t' 'NR == FNR { if ($1 !~ /^#/) k[$1] = 1; next } !($1 in k) { print $1 }' "$CATALOGUE" "$CASES_TSV" | sort -u | tr '\n' ' ')"
CMD_ALL="$(grep -c . "$CMD_TSV")"
CMD_A="$(awk -F'\t' '$2 == "a"' "$CMD_TSV" | grep -c .)"
CMD_PASS="$(awk -F'\t' '$3 == "PASS"' "$CMD_TSV" | grep -c .)"
CMD_PCT=$(( CMD_A > 0 ? CMD_PASS * 100 / CMD_A : 0 ))
ALL_PCT=$(( CMD_ALL > 0 ? CMD_PASS * 100 / CMD_ALL : 0 ))

{
  echo "# gh compatibility report"
  echo
  echo "gh: \`$("$GH" --version | head -1)\`, host \`$HOST\` (GHES path, http_unix_socket). Generated $(date -u +%Y-%m-%dT%H:%M:%SZ)."
  echo "Denominator: see docs/gh-coverage.md (catalogue: tests/gh/commands.tsv)."
  echo
  echo "**PASS $CMD_PASS/$CMD_A forge-relevant gh commands ($CMD_PCT%)**"
  echo
  echo "**$CMD_PASS/$CMD_ALL of all gh commands ($ALL_PCT%)**"
  echo
  echo "**Cases: PASS $PASS/$TOTAL ($PCT%)**"
  echo
  echo "## Commands"
  echo
  echo "| Command | Category | Result | Cases passing | Note |"
  echo "|---------|----------|--------|---------------|------|"
  awk -F'\t' '{ printf "| `gh %s` | %s | %s | %s | %s |\n", $1, $2, $3, $4, $5 }' "$CMD_TSV"
  echo
  echo "## Cases"
  echo
  echo "| # | Case | Command | Result | Exit | Notes |"
  echo "|---|------|---------|--------|------|-------|"
  for i in "${!NAMES[@]}"; do
    note="${NOTES[$i]//|/\\|}"
    echo "| $((i + 1)) | ${NAMES[$i]} | ${KEYS[$i]} | ${RESULTS[$i]} | ${CODES[$i]} | ${note} |"
  done
} >"$REPORT"

printf '%-4s %-42s %-5s %s\n' "#" "case" "res" "notes"
for i in "${!NAMES[@]}"; do
  printf '%-4s %-42s %-5s %s\n' "$((i + 1))" "${NAMES[$i]}" "${RESULTS[$i]}" "${NOTES[$i]}"
done
echo
awk -F'\t' '$3 == "FAIL" || $3 == "UNTESTED" { printf "  %-8s gh %s (%s cases pass)\n", $3, $1, $4 }' "$CMD_TSV"
[ -n "$UNKNOWN" ] && echo "WARNING: cases attributed to commands missing from the catalogue: $UNKNOWN"
echo
echo "Cases: PASS $PASS/$TOTAL ($PCT%)"
echo "PASS $CMD_PASS/$CMD_A forge-relevant gh commands ($CMD_PCT%)"
echo "$CMD_PASS/$CMD_ALL of all gh commands ($ALL_PCT%)"
echo "logs: $WORK/logs   report: $REPORT"
[ "$PASS" = "$TOTAL" ]

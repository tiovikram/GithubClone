# gh compatibility report

gh: `gh version 2.102.0 (2026-09-30)`, host `forgejo.localhost` (GHES path, http_unix_socket). Generated 2026-10-13T00:42:40Z.

**PASS 0/88 (0%)**

| # | Case | Result | Exit | Notes |
|---|------|--------|------|-------|
| 1 | auth status | FAIL | 1 | exit 1 (want 0): forgejo.localhost   X Failed to log in to forgejo.localhost using token (GH_ENTERPRISE_TOKEN)   - Active account: true  |
| 2 | api user (REST GET) | FAIL | 1 | exit 1 (want 0): Not found. gh: HTTP 404  |
| 3 | api graphql query | FAIL | 1 | exit 1 (want 0): gh: HTTP 405  |
| 4 | repo create (GraphQL) | FAIL | 1 | exit 1 (want 0): HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 5 | repo create --add-readme (REST) | FAIL | 1 | exit 1 (want 0): HTTP 404: 404 Not Found (https://forgejo.localhost/api/v3/user/repos)  |
| 6 | repo view | FAIL | 1 | exit 1 (want 0): HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 7 | repo view (README) | FAIL | 1 | exit 1 (want 0): HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 8 | repo view --json | FAIL | 1 | exit 1 (want 0): HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 9 | repo list | FAIL | 1 | exit 1 (want 0): non-200 OK status code: 405 Method Not Allowed body: ""  |
| 10 | repo list OWNER --json | FAIL | 1 | exit 1 (want 0): non-200 OK status code: 405 Method Not Allowed body: ""  |
| 11 | repo edit | FAIL | 1 | exit 1 (want 0): HTTP 404: 404 Not Found (https://forgejo.localhost/api/v3/repos/alice-a47457/demo)  |
| 12 | repo view after edit | FAIL | 1 | exit 1 (want 0): HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 13 | repo clone | FAIL | 1 | exit 1 (want 0): HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 14 | repo clone result | FAIL | 128 | exit 128 (want 0): fatal: cannot change to '/Users/vs/Workspace/GitHubClone/.claude/worktrees/agent-a69095d3dd37d103c/data/bench/gh-forgejo/work/clone': No such file or directory  |
| 15 | repo fork (as other user) | FAIL | 1 | exit 1 (want 0): failed to fork: HTTP 404: 404 Not Found (https://forgejo.localhost/api/v3/repos/alice-a47457/demo/forks)  |
| 16 | repo list --fork (other user) | FAIL | 1 | exit 1 (want 0): non-200 OK status code: 405 Method Not Allowed body: ""  |
| 17 | label create (REST) | FAIL | 1 | exit 1 (want 0): HTTP 404: 404 Not Found (https://forgejo.localhost/api/v3/repos/alice-a47457/demo/labels)  |
| 18 | label create --force (REST) | FAIL | 1 | exit 1 (want 0): HTTP 404: 404 Not Found (https://forgejo.localhost/api/v3/repos/alice-a47457/demo/labels)  |
| 19 | api graphql mutation (createLabel) | FAIL | 1 | exit 1 (want 0): gh: HTTP 405  |
| 20 | label list | FAIL | 1 | exit 1 (want 0): HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 21 | label list --json | FAIL | 1 | exit 1 (want 0): HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 22 | issue create | FAIL | 1 | exit 1 (want 0): non-200 OK status code: 405 Method Not Allowed body: ""  |
| 23 | issue create with metadata | FAIL | 1 | exit 1 (want 0): non-200 OK status code: 405 Method Not Allowed body: ""  |
| 24 | issue create (to delete) | FAIL | 1 | exit 1 (want 0): non-200 OK status code: 405 Method Not Allowed body: ""  |
| 25 | issue list | FAIL | 1 | exit 1 (want 0): HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 26 | issue list --json | FAIL | 1 | exit 1 (want 0): HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 27 | issue list --label (search) | FAIL | 1 | exit 1 (want 0): HTTP 404: 404 Not Found (https://forgejo.localhost/api/v3/meta)  |
| 28 | issue list --search | FAIL | 1 | exit 1 (want 0): HTTP 404: 404 Not Found (https://forgejo.localhost/api/v3/meta)  |
| 29 | issue list --assignee | FAIL | 1 | exit 1 (want 0): HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 30 | issue view | FAIL | 1 | exit 1 (want 0): HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 31 | issue comment | FAIL | 1 | exit 1 (want 0): HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 32 | issue view --comments | FAIL | 1 | exit 1 (want 0): HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 33 | issue view --json | FAIL | 1 | exit 1 (want 0): HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 34 | issue edit | FAIL | 1 | exit 1 (want 0): non-200 OK status code: 405 Method Not Allowed body: ""  |
| 35 | issue edit --remove-label | FAIL | 1 | exit 1 (want 0): non-200 OK status code: 405 Method Not Allowed body: ""  |
| 36 | issue view after edit | FAIL | 1 | exit 1 (want 0): HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 37 | issue close | FAIL | 1 | exit 1 (want 0): HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 38 | issue list --state closed | FAIL | 1 | exit 1 (want 0): HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 39 | issue reopen | FAIL | 1 | exit 1 (want 0): HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 40 | issue status | FAIL | 1 | exit 1 (want 0): non-200 OK status code: 405 Method Not Allowed body: ""  |
| 41 | issue delete | FAIL | 1 | exit 1 (want 0): HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 42 | issue view deleted (404) | FAIL | 1 | output did not match /Could not resolve/: HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 43 | pr create | FAIL | 1 | exit 1 (want 0): HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 44 | pr create (reviewer, label, assignee) | FAIL | 1 | exit 1 (want 0): HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 45 | pr create --draft | FAIL | 1 | exit 1 (want 0): HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 46 | pr create (4th) | FAIL | 1 | exit 1 (want 0): HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 47 | pr create duplicate fails | FAIL | 1 | output did not match /already exists/: HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 48 | pr list | FAIL | 1 | exit 1 (want 0): HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 49 | pr list --json | FAIL | 1 | exit 1 (want 0): HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 50 | pr list --author (search) | FAIL | 1 | exit 1 (want 0): HTTP 404: 404 Not Found (https://forgejo.localhost/api/v3/meta)  |
| 51 | pr list --draft (search) | FAIL | 1 | exit 1 (want 0): HTTP 404: 404 Not Found (https://forgejo.localhost/api/v3/meta)  |
| 52 | pr view | FAIL | 1 | exit 1 (want 0): HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 53 | pr view --json | FAIL | 1 | exit 1 (want 0): HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 54 | pr view reviewers | FAIL | 1 | exit 1 (want 0): HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 55 | pr diff | FAIL | 1 | exit 1 (want 0): HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 56 | pr diff --name-only | FAIL | 1 | exit 1 (want 0): HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 57 | pr comment | FAIL | 1 | exit 1 (want 0): HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 58 | pr review --comment | FAIL | 1 | exit 1 (want 0): HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 59 | pr review --approve (other user) | FAIL | 1 | exit 1 (want 0): HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 60 | pr review --request-changes (other user) | FAIL | 1 | exit 1 (want 0): HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 61 | pr view review decision | FAIL | 1 | exit 1 (want 0): HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 62 | pr edit | FAIL | 1 | exit 1 (want 0): non-200 OK status code: 405 Method Not Allowed body: ""  |
| 63 | pr ready --undo | FAIL | 1 | exit 1 (want 0): HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 64 | pr ready | FAIL | 1 | exit 1 (want 0): HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 65 | pr view after ready | FAIL | 1 | exit 1 (want 0): HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 66 | pr ready (undo the undo) | FAIL | 1 | exit 1 (want 0): HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 67 | pr checks | FAIL | 1 | exit 1 (want 0): HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 68 | pr checkout | FAIL | 1 | exit 1 (want 0): /Users/vs/Workspace/GitHubClone/.claude/worktrees/agent-a69095d3dd37d103c/tests/gh/run.sh: line 134: cd: /Users/vs/Workspace/GitHubClone/.claude/worktrees/agent |
| 69 | pr checkout result | FAIL | 1 | exit 1 (want 0): /Users/vs/Workspace/GitHubClone/.claude/worktrees/agent-a69095d3dd37d103c/tests/gh/run.sh: line 134: cd: /Users/vs/Workspace/GitHubClone/.claude/worktrees/agent |
| 70 | pr status | FAIL | 1 | exit 1 (want 0): /Users/vs/Workspace/GitHubClone/.claude/worktrees/agent-a69095d3dd37d103c/tests/gh/run.sh: line 134: cd: /Users/vs/Workspace/GitHubClone/.claude/worktrees/agent |
| 71 | pr view (current branch) | FAIL | 1 | exit 1 (want 0): /Users/vs/Workspace/GitHubClone/.claude/worktrees/agent-a69095d3dd37d103c/tests/gh/run.sh: line 134: cd: /Users/vs/Workspace/GitHubClone/.claude/worktrees/agent |
| 72 | pr merge --merge --delete-branch | FAIL | 1 | exit 1 (want 0): /Users/vs/Workspace/GitHubClone/.claude/worktrees/agent-a69095d3dd37d103c/tests/gh/run.sh: line 134: cd: /Users/vs/Workspace/GitHubClone/.claude/worktrees/agent |
| 73 | pr merge result (local main) | FAIL | 1 | exit 1 (want 0): /Users/vs/Workspace/GitHubClone/.claude/worktrees/agent-a69095d3dd37d103c/tests/gh/run.sh: line 134: cd: /Users/vs/Workspace/GitHubClone/.claude/worktrees/agent |
| 74 | pr view merged | FAIL | 1 | exit 1 (want 0): HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 75 | pr close | FAIL | 1 | exit 1 (want 0): HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 76 | pr reopen | FAIL | 1 | exit 1 (want 0): HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 77 | pr merge --squash | FAIL | 1 | exit 1 (want 0): non-200 OK status code: 405 Method Not Allowed body: ""  |
| 78 | pr merge --rebase | FAIL | 1 | exit 1 (want 0): non-200 OK status code: 405 Method Not Allowed body: ""  |
| 79 | pr list --state merged | FAIL | 1 | exit 1 (want 0): HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 80 | pr merge result (remote log) | FAIL | 1 | exit 1 (want 0): invalid character 'N' looking for beginning of value  |
| 81 | release create | FAIL | 1 | exit 1 (want 0): HTTP 404: 404 Not Found (https://forgejo.localhost/api/v3/repos/alice-a47457/demo/releases)  |
| 82 | release list | FAIL | 1 | exit 1 (want 0): non-200 OK status code: 405 Method Not Allowed body: ""  |
| 83 | release view | FAIL | 1 | exit 1 (want 0): release not found  |
| 84 | api POST -f (REST) | FAIL | 1 | exit 1 (want 0): Not found. gh: HTTP 404  |
| 85 | api --paginate (REST) | FAIL | 1 | exit 1 (want 0): invalid character 'N' looking for beginning of value  |
| 86 | api graphql --paginate | FAIL | 1 | exit 1 (want 0): gh: HTTP 405  |
| 87 | repo delete | FAIL | 1 | exit 1 (want 0): HTTP 404: 404 Not Found (https://forgejo.localhost/api/v3/repos/alice-a47457/apirepo)  |
| 88 | repo view deleted (404) | FAIL | 1 | output did not match /Could not resolve/: HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |

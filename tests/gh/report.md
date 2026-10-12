# gh compatibility report

gh: `gh version 2.102.0 (2026-09-30)`, host `forge.localhost` (GHES path, http_unix_socket). Generated 2026-10-12T22:17:05Z.

**PASS 85/88 (96%)**

| # | Case | Result | Exit | Notes |
|---|------|--------|------|-------|
| 1 | auth status | PASS | 0 |  |
| 2 | api user (REST GET) | PASS | 0 |  |
| 3 | api graphql query | PASS | 0 |  |
| 4 | repo create (GraphQL) | PASS | 0 |  |
| 5 | repo create --add-readme (REST) | PASS | 0 |  |
| 6 | repo view | PASS | 0 |  |
| 7 | repo view (README) | PASS | 0 |  |
| 8 | repo view --json | PASS | 0 |  |
| 9 | repo list | PASS | 0 |  |
| 10 | repo list OWNER --json | PASS | 0 |  |
| 11 | repo edit | PASS | 0 |  |
| 12 | repo view after edit | PASS | 0 |  |
| 13 | repo clone | PASS | 0 |  |
| 14 | repo clone result | PASS | 0 |  |
| 15 | repo fork (as other user) | PASS | 0 |  |
| 16 | repo list --fork (other user) | PASS | 0 |  |
| 17 | label create (REST) | PASS | 0 |  |
| 18 | label create --force (REST) | PASS | 0 |  |
| 19 | api graphql mutation (createLabel) | PASS | 0 |  |
| 20 | label list | PASS | 0 |  |
| 21 | label list --json | PASS | 0 |  |
| 22 | issue create | PASS | 0 |  |
| 23 | issue create with metadata | PASS | 0 |  |
| 24 | issue create (to delete) | PASS | 0 |  |
| 25 | issue list | PASS | 0 |  |
| 26 | issue list --json | PASS | 0 |  |
| 27 | issue list --label (search) | PASS | 0 |  |
| 28 | issue list --search | PASS | 0 |  |
| 29 | issue list --assignee | PASS | 0 |  |
| 30 | issue view | PASS | 0 |  |
| 31 | issue comment | PASS | 0 |  |
| 32 | issue view --comments | PASS | 0 |  |
| 33 | issue view --json | PASS | 0 |  |
| 34 | issue edit | PASS | 0 |  |
| 35 | issue edit --remove-label | PASS | 0 |  |
| 36 | issue view after edit | PASS | 0 |  |
| 37 | issue close | PASS | 0 |  |
| 38 | issue list --state closed | PASS | 0 |  |
| 39 | issue reopen | PASS | 0 |  |
| 40 | issue status | PASS | 0 |  |
| 41 | issue delete | PASS | 0 |  |
| 42 | issue view deleted (404) | PASS | 1 |  |
| 43 | pr create | PASS | 0 |  |
| 44 | pr create (reviewer, label, assignee) | PASS | 0 |  |
| 45 | pr create --draft | PASS | 0 |  |
| 46 | pr create (4th) | PASS | 0 |  |
| 47 | pr create duplicate fails | PASS | 1 |  |
| 48 | pr list | PASS | 0 |  |
| 49 | pr list --json | PASS | 0 |  |
| 50 | pr list --author (search) | PASS | 0 |  |
| 51 | pr list --draft (search) | PASS | 0 |  |
| 52 | pr view | PASS | 0 |  |
| 53 | pr view --json | PASS | 0 |  |
| 54 | pr view reviewers | PASS | 0 |  |
| 55 | pr diff | PASS | 0 |  |
| 56 | pr diff --name-only | PASS | 0 |  |
| 57 | pr comment | PASS | 0 |  |
| 58 | pr review --comment | PASS | 0 |  |
| 59 | pr review --approve (other user) | PASS | 0 |  |
| 60 | pr review --request-changes (other user) | PASS | 0 |  |
| 61 | pr view review decision | PASS | 0 |  |
| 62 | pr edit | PASS | 0 |  |
| 63 | pr ready --undo | PASS | 0 |  |
| 64 | pr ready | PASS | 0 |  |
| 65 | pr view after ready | PASS | 0 |  |
| 66 | pr ready (undo the undo) | PASS | 0 |  |
| 67 | pr checks | PASS | 0 |  |
| 68 | pr checkout | PASS | 0 |  |
| 69 | pr checkout result | PASS | 0 |  |
| 70 | pr status | PASS | 0 |  |
| 71 | pr view (current branch) | PASS | 0 |  |
| 72 | pr merge --merge --delete-branch | PASS | 0 |  |
| 73 | pr merge result (local main) | PASS | 0 |  |
| 74 | pr view merged | PASS | 0 |  |
| 75 | pr close | PASS | 0 |  |
| 76 | pr reopen | PASS | 0 |  |
| 77 | pr merge --squash | PASS | 0 |  |
| 78 | pr merge --rebase | PASS | 0 |  |
| 79 | pr list --state merged | PASS | 0 |  |
| 80 | pr merge result (remote log) | PASS | 0 |  |
| 81 | release create | FAIL | 1 | exit 1 (want 0): HTTP 404: Not Found (https://forge.localhost/api/v3/repos/alice-xu2q3p/demo/releases)  |
| 82 | release list | FAIL | 0 | output did not match /v1.0/:  |
| 83 | release view | FAIL | 1 | exit 1 (want 0): release not found  |
| 84 | api POST -f (REST) | PASS | 0 |  |
| 85 | api --paginate (REST) | PASS | 0 |  |
| 86 | api graphql --paginate | PASS | 0 |  |
| 87 | repo delete | PASS | 0 |  |
| 88 | repo view deleted (404) | PASS | 1 |  |

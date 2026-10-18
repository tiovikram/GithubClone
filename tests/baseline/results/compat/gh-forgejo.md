# gh compatibility report

gh: `gh version 2.102.0 (2026-09-30)`, host `forgejo.localhost` (GHES path, http_unix_socket). Generated 2026-10-18T08:22:40Z.
Denominator: see docs/gh-coverage.md (catalogue: tests/gh/commands.tsv).

**PASS 11/166 forge-relevant gh commands (6%)**

**11/197 of all gh commands (5%)**

**Cases: PASS 17/349 (4%)**

## Commands

| Command | Category | Result | Cases passing | Note |
|---------|----------|--------|---------------|------|
| `gh agent-task create` | b | n/a | 0/0 | Copilot coding agent: github.com-only service (api.githubcopilot.com) |
| `gh agent-task list` | b | n/a | 0/0 | Copilot coding agent: github.com-only service |
| `gh agent-task view` | b | n/a | 0/0 | Copilot coding agent: github.com-only service |
| `gh alias delete` | a | PASS | 1/1 | local-only |
| `gh alias import` | a | FAIL | 1/2 | local-only |
| `gh alias list` | a | PASS | 1/1 | local-only |
| `gh alias set` | a | FAIL | 1/2 | local-only |
| `gh api` | a | FAIL | 0/13 |  |
| `gh attestation download` | b | n/a | 0/0 | Sigstore/GitHub artifact attestations service |
| `gh attestation trusted-root` | b | n/a | 0/0 | Sigstore TUF trusted root (github.com / public-good Sigstore) |
| `gh attestation verify` | b | n/a | 0/0 | Sigstore/GitHub artifact attestations service |
| `gh auth login` | a | FAIL | 0/2 |  |
| `gh auth logout` | a | FAIL | 0/2 | local-only |
| `gh auth refresh` | a | FAIL | 0/2 | needs the OAuth device/web flow |
| `gh auth setup-git` | a | PASS | 2/2 | local-only |
| `gh auth status` | a | FAIL | 0/3 |  |
| `gh auth switch` | a | FAIL | 0/1 | local-only |
| `gh auth token` | a | FAIL | 0/2 | local-only |
| `gh browse` | a | FAIL | 0/6 | tested with --no-browser (prints the URL) |
| `gh cache delete` | a | FAIL | 0/1 | Actions caches (forge has none) |
| `gh cache list` | a | FAIL | 0/2 | Actions caches (forge has none) |
| `gh codespace code` | b | n/a | 0/0 | GitHub Codespaces service |
| `gh codespace cp` | b | n/a | 0/0 | GitHub Codespaces service |
| `gh codespace create` | b | n/a | 0/0 | GitHub Codespaces service |
| `gh codespace delete` | b | n/a | 0/0 | GitHub Codespaces service |
| `gh codespace edit` | b | n/a | 0/0 | GitHub Codespaces service |
| `gh codespace jupyter` | b | n/a | 0/0 | GitHub Codespaces service |
| `gh codespace list` | b | n/a | 0/0 | GitHub Codespaces service |
| `gh codespace logs` | b | n/a | 0/0 | GitHub Codespaces service |
| `gh codespace ports` | b | n/a | 0/0 | GitHub Codespaces service |
| `gh codespace ports forward` | b | n/a | 0/0 | GitHub Codespaces service |
| `gh codespace ports visibility` | b | n/a | 0/0 | GitHub Codespaces service |
| `gh codespace rebuild` | b | n/a | 0/0 | GitHub Codespaces service |
| `gh codespace ssh` | b | n/a | 0/0 | GitHub Codespaces service |
| `gh codespace stop` | b | n/a | 0/0 | GitHub Codespaces service |
| `gh codespace view` | b | n/a | 0/0 | GitHub Codespaces service |
| `gh completion` | a | PASS | 1/1 | local-only |
| `gh config clear-cache` | a | PASS | 1/1 | local-only |
| `gh config get` | a | PASS | 1/1 | local-only |
| `gh config list` | a | PASS | 1/1 | local-only |
| `gh config set` | a | PASS | 1/1 | local-only |
| `gh copilot` | b | n/a | 0/0 | downloads and runs the GitHub Copilot CLI |
| `gh discussion comment` | a | FAIL | 0/1 | Discussions |
| `gh discussion create` | a | FAIL | 0/2 | Discussions |
| `gh discussion edit` | a | FAIL | 0/2 | Discussions |
| `gh discussion list` | a | FAIL | 0/3 | Discussions |
| `gh discussion view` | a | FAIL | 0/2 | Discussions |
| `gh extension browse` | b | n/a | 0/0 | full-screen TUI; needs a terminal, cannot run non-interactively |
| `gh extension create` | a | PASS | 2/2 | local-only (git) |
| `gh extension exec` | a | FAIL | 0/1 | local-only |
| `gh extension install` | a | FAIL | 0/1 | clones from the forge |
| `gh extension list` | a | FAIL | 0/1 | local-only |
| `gh extension remove` | a | FAIL | 0/1 | local-only |
| `gh extension search` | a | FAIL | 0/1 | repository search (topic:gh-extension) |
| `gh extension upgrade` | a | FAIL | 0/2 | fetches from the forge |
| `gh gist clone` | a | FAIL | 0/2 |  |
| `gh gist create` | a | FAIL | 0/2 |  |
| `gh gist delete` | a | FAIL | 0/2 |  |
| `gh gist edit` | a | FAIL | 0/4 |  |
| `gh gist list` | a | FAIL | 0/2 |  |
| `gh gist rename` | a | FAIL | 0/2 |  |
| `gh gist view` | a | FAIL | 0/2 |  |
| `gh gpg-key add` | a | FAIL | 0/1 |  |
| `gh gpg-key delete` | a | FAIL | 0/2 |  |
| `gh gpg-key list` | a | FAIL | 0/1 |  |
| `gh issue close` | a | FAIL | 0/2 |  |
| `gh issue comment` | a | FAIL | 0/2 |  |
| `gh issue create` | a | FAIL | 0/5 |  |
| `gh issue delete` | a | FAIL | 0/1 |  |
| `gh issue develop` | a | FAIL | 0/3 | linked branches |
| `gh issue edit` | a | FAIL | 0/3 |  |
| `gh issue list` | a | FAIL | 0/8 |  |
| `gh issue lock` | a | FAIL | 0/2 |  |
| `gh issue pin` | a | FAIL | 0/2 |  |
| `gh issue reopen` | a | FAIL | 0/2 |  |
| `gh issue status` | a | FAIL | 0/2 |  |
| `gh issue transfer` | a | FAIL | 0/3 |  |
| `gh issue unlock` | a | FAIL | 0/2 |  |
| `gh issue unpin` | a | FAIL | 0/2 |  |
| `gh issue view` | a | FAIL | 0/5 |  |
| `gh label clone` | a | FAIL | 0/2 |  |
| `gh label create` | a | FAIL | 0/2 |  |
| `gh label delete` | a | FAIL | 0/2 |  |
| `gh label edit` | a | FAIL | 0/2 |  |
| `gh label list` | a | FAIL | 0/2 |  |
| `gh licenses` | a | PASS | 1/1 | local-only |
| `gh org list` | a | FAIL | 0/1 |  |
| `gh pr checkout` | a | FAIL | 0/2 |  |
| `gh pr checks` | a | FAIL | 0/2 |  |
| `gh pr close` | a | FAIL | 0/1 |  |
| `gh pr comment` | a | FAIL | 0/2 |  |
| `gh pr create` | a | FAIL | 0/6 |  |
| `gh pr diff` | a | FAIL | 0/3 |  |
| `gh pr edit` | a | FAIL | 0/2 |  |
| `gh pr list` | a | FAIL | 0/7 |  |
| `gh pr lock` | a | FAIL | 0/2 |  |
| `gh pr merge` | a | FAIL | 0/4 |  |
| `gh pr ready` | a | FAIL | 0/3 |  |
| `gh pr reopen` | a | FAIL | 0/1 |  |
| `gh pr revert` | a | FAIL | 0/3 |  |
| `gh pr review` | a | FAIL | 0/4 |  |
| `gh pr status` | a | FAIL | 0/2 |  |
| `gh pr unlock` | a | FAIL | 0/2 |  |
| `gh pr update-branch` | a | FAIL | 0/4 |  |
| `gh pr view` | a | FAIL | 0/8 |  |
| `gh preview prompter` | b | n/a | 0/0 | developer preview of gh's own TTY prompts; no host interaction, needs a terminal |
| `gh project close` | a | FAIL | 0/2 | Projects (v2) |
| `gh project copy` | a | FAIL | 0/1 | Projects (v2) |
| `gh project create` | a | FAIL | 0/1 | Projects (v2) |
| `gh project delete` | a | FAIL | 0/2 | Projects (v2) |
| `gh project edit` | a | FAIL | 0/1 | Projects (v2) |
| `gh project field-create` | a | FAIL | 0/2 | Projects (v2) |
| `gh project field-delete` | a | FAIL | 1/2 | Projects (v2) |
| `gh project field-list` | a | FAIL | 0/1 | Projects (v2) |
| `gh project item-add` | a | FAIL | 0/1 | Projects (v2) |
| `gh project item-archive` | a | FAIL | 0/2 | Projects (v2) |
| `gh project item-create` | a | FAIL | 0/1 | Projects (v2) |
| `gh project item-delete` | a | FAIL | 0/2 | Projects (v2) |
| `gh project item-edit` | a | FAIL | 0/3 | Projects (v2) |
| `gh project item-list` | a | FAIL | 0/2 | Projects (v2) |
| `gh project link` | a | FAIL | 0/2 | Projects (v2) |
| `gh project list` | a | FAIL | 0/2 | Projects (v2) |
| `gh project mark-template` | a | FAIL | 0/2 | Projects (v2) |
| `gh project unlink` | a | FAIL | 0/2 | Projects (v2) |
| `gh project view` | a | FAIL | 0/1 | Projects (v2) |
| `gh release create` | a | FAIL | 0/3 |  |
| `gh release delete` | a | FAIL | 1/2 |  |
| `gh release delete-asset` | a | FAIL | 0/2 |  |
| `gh release download` | a | FAIL | 0/4 |  |
| `gh release edit` | a | FAIL | 0/2 |  |
| `gh release list` | a | FAIL | 0/2 |  |
| `gh release upload` | a | FAIL | 0/1 |  |
| `gh release verify` | b | n/a | 0/0 | verifies Sigstore release attestations |
| `gh release verify-asset` | b | n/a | 0/0 | verifies Sigstore release attestations |
| `gh release view` | a | FAIL | 0/2 |  |
| `gh repo archive` | a | FAIL | 0/2 |  |
| `gh repo autolink create` | a | FAIL | 0/1 |  |
| `gh repo autolink delete` | a | FAIL | 0/1 |  |
| `gh repo autolink list` | a | FAIL | 0/2 |  |
| `gh repo autolink view` | a | FAIL | 0/1 |  |
| `gh repo clone` | a | FAIL | 0/2 |  |
| `gh repo create` | a | FAIL | 0/2 |  |
| `gh repo delete` | a | FAIL | 0/1 |  |
| `gh repo deploy-key add` | a | FAIL | 0/1 |  |
| `gh repo deploy-key delete` | a | FAIL | 0/1 |  |
| `gh repo deploy-key list` | a | FAIL | 0/2 |  |
| `gh repo edit` | a | FAIL | 0/3 |  |
| `gh repo fork` | a | FAIL | 0/1 |  |
| `gh repo gitignore list` | a | FAIL | 0/1 |  |
| `gh repo gitignore view` | a | FAIL | 0/1 |  |
| `gh repo license list` | a | FAIL | 0/1 |  |
| `gh repo license view` | a | FAIL | 0/1 |  |
| `gh repo list` | a | FAIL | 0/5 |  |
| `gh repo read-dir` | a | FAIL | 0/2 |  |
| `gh repo read-file` | a | FAIL | 0/2 |  |
| `gh repo rename` | a | FAIL | 0/2 |  |
| `gh repo set-default` | a | FAIL | 0/2 | local-only (git config) |
| `gh repo sync` | a | FAIL | 0/4 |  |
| `gh repo unarchive` | a | FAIL | 0/2 |  |
| `gh repo view` | a | FAIL | 0/6 |  |
| `gh ruleset check` | a | FAIL | 0/1 |  |
| `gh ruleset list` | a | FAIL | 0/1 |  |
| `gh ruleset view` | a | FAIL | 0/1 |  |
| `gh run cancel` | a | FAIL | 0/1 | Actions runs |
| `gh run delete` | a | FAIL | 0/1 | Actions runs |
| `gh run download` | a | FAIL | 0/1 | Actions runs |
| `gh run list` | a | FAIL | 0/2 | Actions runs |
| `gh run rerun` | a | FAIL | 0/1 | Actions runs |
| `gh run view` | a | FAIL | 0/2 | Actions runs |
| `gh run watch` | a | FAIL | 0/1 | Actions runs |
| `gh search code` | a | FAIL | 0/2 |  |
| `gh search commits` | a | FAIL | 0/2 |  |
| `gh search issues` | a | FAIL | 0/2 |  |
| `gh search prs` | a | FAIL | 0/2 |  |
| `gh search repos` | a | FAIL | 0/2 |  |
| `gh secret delete` | a | FAIL | 0/2 | Actions secrets |
| `gh secret list` | a | FAIL | 0/2 | Actions secrets |
| `gh secret set` | a | FAIL | 0/1 | Actions secrets |
| `gh skill install` | b | n/a | 0/0 | gh refuses non-github.com hosts (source.ValidateSupportedHost: "GitHub Skills does not currently support GitHub Enterprise Server") |
| `gh skill list` | a | PASS | 1/1 | local-only (lists installed skills) |
| `gh skill preview` | b | n/a | 0/0 | gh refuses non-github.com hosts (source.ValidateSupportedHost: "GitHub Skills does not currently support GitHub Enterprise Server") |
| `gh skill publish` | b | n/a | 0/0 | gh refuses non-github.com hosts (source.ValidateSupportedHost: "GitHub Skills does not currently support GitHub Enterprise Server") |
| `gh skill search` | b | n/a | 0/0 | gh refuses non-github.com hosts (source.ValidateSupportedHost: "GitHub Skills does not currently support GitHub Enterprise Server") |
| `gh skill update` | b | n/a | 0/0 | gh refuses non-github.com hosts (source.ValidateSupportedHost: "GitHub Skills does not currently support GitHub Enterprise Server") |
| `gh ssh-key add` | a | FAIL | 0/2 |  |
| `gh ssh-key delete` | a | FAIL | 0/2 |  |
| `gh ssh-key list` | a | FAIL | 0/1 |  |
| `gh status` | a | FAIL | 0/2 |  |
| `gh variable delete` | a | FAIL | 0/1 | Actions variables |
| `gh variable get` | a | FAIL | 0/1 | Actions variables |
| `gh variable list` | a | FAIL | 0/2 | Actions variables |
| `gh variable set` | a | FAIL | 0/3 | Actions variables |
| `gh workflow disable` | a | FAIL | 0/2 | Actions workflows |
| `gh workflow enable` | a | FAIL | 0/2 | Actions workflows |
| `gh workflow list` | a | FAIL | 0/2 | Actions workflows |
| `gh workflow run` | a | FAIL | 0/1 | Actions workflows |
| `gh workflow view` | a | FAIL | 0/1 | Actions workflows |

## Cases

| # | Case | Command | Result | Exit | Notes |
|---|------|---------|--------|------|-------|
| 1 | auth status | auth status | FAIL | 1 | exit 1 (want 0): forgejo.localhost   X Failed to log in to forgejo.localhost using token (GH_ENTERPRISE_TOKEN)   - Active account: true  |
| 2 | api user (REST GET) | api | FAIL | 1 | exit 1 (want 0): Not found. gh: HTTP 404  |
| 3 | api graphql query | api | FAIL | 1 | exit 1 (want 0): gh: HTTP 405  |
| 4 | repo create (GraphQL) | repo create | FAIL | 1 | exit 1 (want 0): HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 5 | repo create --add-readme (REST) | repo create | FAIL | 1 | exit 1 (want 0): HTTP 404: 404 Not Found (https://forgejo.localhost/api/v3/user/repos)  |
| 6 | repo view | repo view | FAIL | 1 | exit 1 (want 0): HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 7 | repo view (README) | repo view | FAIL | 1 | exit 1 (want 0): HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 8 | repo view --json | repo view | FAIL | 1 | exit 1 (want 0): HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 9 | repo list | repo list | FAIL | 1 | exit 1 (want 0): non-200 OK status code: 405 Method Not Allowed body: ""  |
| 10 | repo list OWNER --json | repo list | FAIL | 1 | exit 1 (want 0): non-200 OK status code: 405 Method Not Allowed body: ""  |
| 11 | repo edit | repo edit | FAIL | 1 | exit 1 (want 0): HTTP 404: 404 Not Found (https://forgejo.localhost/api/v3/repos/alice-e94ba7/demo)  |
| 12 | repo view after edit | repo view | FAIL | 1 | exit 1 (want 0): HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 13 | repo clone | repo clone | FAIL | 1 | exit 1 (want 0): HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 14 | repo clone result | repo clone | FAIL | 128 | exit 128 (want 0): fatal: cannot change to '/Users/vs/Workspace/GitHubClone/.claude/worktrees/agent-a0df397a2e1901171/data/bench/gh-forgejo/work/clone': No such file or directory  |
| 15 | repo fork (as other user) | repo fork | FAIL | 1 | exit 1 (want 0): failed to fork: HTTP 404: 404 Not Found (https://forgejo.localhost/api/v3/repos/alice-e94ba7/demo/forks)  |
| 16 | repo list --fork (other user) | repo list | FAIL | 1 | exit 1 (want 0): non-200 OK status code: 405 Method Not Allowed body: ""  |
| 17 | label create (REST) | label create | FAIL | 1 | exit 1 (want 0): HTTP 404: 404 Not Found (https://forgejo.localhost/api/v3/repos/alice-e94ba7/demo/labels)  |
| 18 | label create --force (REST) | label create | FAIL | 1 | exit 1 (want 0): HTTP 404: 404 Not Found (https://forgejo.localhost/api/v3/repos/alice-e94ba7/demo/labels)  |
| 19 | api graphql mutation (createLabel) | api | FAIL | 1 | exit 1 (want 0): gh: HTTP 405  |
| 20 | label list | label list | FAIL | 1 | exit 1 (want 0): HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 21 | label list --json | label list | FAIL | 1 | exit 1 (want 0): HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 22 | issue create | issue create | FAIL | 1 | exit 1 (want 0): non-200 OK status code: 405 Method Not Allowed body: ""  |
| 23 | issue create with metadata | issue create | FAIL | 1 | exit 1 (want 0): non-200 OK status code: 405 Method Not Allowed body: ""  |
| 24 | issue create (to delete) | issue create | FAIL | 1 | exit 1 (want 0): non-200 OK status code: 405 Method Not Allowed body: ""  |
| 25 | issue list | issue list | FAIL | 1 | exit 1 (want 0): HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 26 | issue list --json | issue list | FAIL | 1 | exit 1 (want 0): HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 27 | issue list --label (search) | issue list | FAIL | 1 | exit 1 (want 0): HTTP 404: 404 Not Found (https://forgejo.localhost/api/v3/meta)  |
| 28 | issue list --search | issue list | FAIL | 1 | exit 1 (want 0): HTTP 404: 404 Not Found (https://forgejo.localhost/api/v3/meta)  |
| 29 | issue list --assignee | issue list | FAIL | 1 | exit 1 (want 0): HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 30 | issue view | issue view | FAIL | 1 | exit 1 (want 0): HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 31 | issue comment | issue comment | FAIL | 1 | exit 1 (want 0): HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 32 | issue view --comments | issue view | FAIL | 1 | exit 1 (want 0): HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 33 | issue view --json | issue view | FAIL | 1 | exit 1 (want 0): HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 34 | issue edit | issue edit | FAIL | 1 | exit 1 (want 0): non-200 OK status code: 405 Method Not Allowed body: ""  |
| 35 | issue edit --remove-label | issue edit | FAIL | 1 | exit 1 (want 0): non-200 OK status code: 405 Method Not Allowed body: ""  |
| 36 | issue view after edit | issue view | FAIL | 1 | exit 1 (want 0): HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 37 | issue close | issue close | FAIL | 1 | exit 1 (want 0): HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 38 | issue list --state closed | issue list | FAIL | 1 | exit 1 (want 0): HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 39 | issue reopen | issue reopen | FAIL | 1 | exit 1 (want 0): HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 40 | issue status | issue status | FAIL | 1 | exit 1 (want 0): non-200 OK status code: 405 Method Not Allowed body: ""  |
| 41 | issue delete | issue delete | FAIL | 1 | exit 1 (want 0): HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 42 | issue view deleted (404) | issue view | FAIL | 1 | output did not match /Could not resolve/: HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 43 | pr create | pr create | FAIL | 1 | exit 1 (want 0): HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 44 | pr create (reviewer, label, assignee) | pr create | FAIL | 1 | exit 1 (want 0): HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 45 | pr create --draft | pr create | FAIL | 1 | exit 1 (want 0): HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 46 | pr create (4th) | pr create | FAIL | 1 | exit 1 (want 0): HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 47 | pr create duplicate fails | pr create | FAIL | 1 | output did not match /already exists/: HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 48 | pr list | pr list | FAIL | 1 | exit 1 (want 0): HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 49 | pr list --json | pr list | FAIL | 1 | exit 1 (want 0): HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 50 | pr list --author (search) | pr list | FAIL | 1 | exit 1 (want 0): HTTP 404: 404 Not Found (https://forgejo.localhost/api/v3/meta)  |
| 51 | pr list --draft (search) | pr list | FAIL | 1 | exit 1 (want 0): HTTP 404: 404 Not Found (https://forgejo.localhost/api/v3/meta)  |
| 52 | pr view | pr view | FAIL | 1 | exit 1 (want 0): HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 53 | pr view --json | pr view | FAIL | 1 | exit 1 (want 0): HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 54 | pr view reviewers | pr view | FAIL | 1 | exit 1 (want 0): HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 55 | pr diff | pr diff | FAIL | 1 | exit 1 (want 0): HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 56 | pr diff --name-only | pr diff | FAIL | 1 | exit 1 (want 0): HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 57 | pr comment | pr comment | FAIL | 1 | exit 1 (want 0): HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 58 | pr review --comment | pr review | FAIL | 1 | exit 1 (want 0): HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 59 | pr review --approve (other user) | pr review | FAIL | 1 | exit 1 (want 0): HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 60 | pr review --request-changes (other user) | pr review | FAIL | 1 | exit 1 (want 0): HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 61 | pr view review decision | pr view | FAIL | 1 | exit 1 (want 0): HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 62 | pr edit | pr edit | FAIL | 1 | exit 1 (want 0): non-200 OK status code: 405 Method Not Allowed body: ""  |
| 63 | pr ready --undo | pr ready | FAIL | 1 | exit 1 (want 0): HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 64 | pr ready | pr ready | FAIL | 1 | exit 1 (want 0): HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 65 | pr view after ready | pr view | FAIL | 1 | exit 1 (want 0): HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 66 | pr ready (undo the undo) | pr ready | FAIL | 1 | exit 1 (want 0): HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 67 | pr checks | pr checks | FAIL | 1 | exit 1 (want 0): HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 68 | pr checkout | pr checkout | FAIL | 1 | exit 1 (want 0): /Users/vs/Workspace/GitHubClone/.claude/worktrees/agent-a0df397a2e1901171/tests/gh/run.sh: line 164: cd: /Users/vs/Workspace/GitHubClone/.claude/worktrees/agent |
| 69 | pr checkout result | pr checkout | FAIL | 1 | exit 1 (want 0): /Users/vs/Workspace/GitHubClone/.claude/worktrees/agent-a0df397a2e1901171/tests/gh/run.sh: line 164: cd: /Users/vs/Workspace/GitHubClone/.claude/worktrees/agent |
| 70 | pr status | pr status | FAIL | 1 | exit 1 (want 0): /Users/vs/Workspace/GitHubClone/.claude/worktrees/agent-a0df397a2e1901171/tests/gh/run.sh: line 164: cd: /Users/vs/Workspace/GitHubClone/.claude/worktrees/agent |
| 71 | pr view (current branch) | pr view | FAIL | 1 | exit 1 (want 0): /Users/vs/Workspace/GitHubClone/.claude/worktrees/agent-a0df397a2e1901171/tests/gh/run.sh: line 164: cd: /Users/vs/Workspace/GitHubClone/.claude/worktrees/agent |
| 72 | pr merge --merge --delete-branch | pr merge | FAIL | 1 | exit 1 (want 0): /Users/vs/Workspace/GitHubClone/.claude/worktrees/agent-a0df397a2e1901171/tests/gh/run.sh: line 164: cd: /Users/vs/Workspace/GitHubClone/.claude/worktrees/agent |
| 73 | pr merge result (local main) | pr merge | FAIL | 1 | exit 1 (want 0): /Users/vs/Workspace/GitHubClone/.claude/worktrees/agent-a0df397a2e1901171/tests/gh/run.sh: line 164: cd: /Users/vs/Workspace/GitHubClone/.claude/worktrees/agent |
| 74 | pr view merged | pr view | FAIL | 1 | exit 1 (want 0): HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 75 | pr close | pr close | FAIL | 1 | exit 1 (want 0): HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 76 | pr reopen | pr reopen | FAIL | 1 | exit 1 (want 0): HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 77 | pr merge --squash | pr merge | FAIL | 1 | exit 1 (want 0): non-200 OK status code: 405 Method Not Allowed body: ""  |
| 78 | pr merge --rebase | pr merge | FAIL | 1 | exit 1 (want 0): non-200 OK status code: 405 Method Not Allowed body: ""  |
| 79 | pr list --state merged | pr list | FAIL | 1 | exit 1 (want 0): HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 80 | pr merge result (remote log) | api | FAIL | 1 | exit 1 (want 0): invalid character 'N' looking for beginning of value  |
| 81 | release create | release create | FAIL | 1 | exit 1 (want 0): HTTP 404: 404 Not Found (https://forgejo.localhost/api/v3/repos/alice-e94ba7/demo/releases)  |
| 82 | release list | release list | FAIL | 1 | exit 1 (want 0): non-200 OK status code: 405 Method Not Allowed body: ""  |
| 83 | release view | release view | FAIL | 1 | exit 1 (want 0): release not found  |
| 84 | api POST -f (REST) | api | FAIL | 1 | exit 1 (want 0): Not found. gh: HTTP 404  |
| 85 | api --paginate (REST) | api | FAIL | 1 | exit 1 (want 0): invalid character 'N' looking for beginning of value  |
| 86 | api graphql --paginate | api | FAIL | 1 | exit 1 (want 0): gh: HTTP 405  |
| 87 | repo delete | repo delete | FAIL | 1 | exit 1 (want 0): HTTP 404: 404 Not Found (https://forgejo.localhost/api/v3/repos/alice-e94ba7/apirepo)  |
| 88 | repo view deleted (404) | repo view | FAIL | 1 | output did not match /Could not resolve/: HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 89 | auth login --with-token | auth login | FAIL | 1 | exit 1 (want 0): error validating token: HTTP 404: 404 Not Found (https://forgejo.localhost/api/v3/)  |
| 90 | auth status (stored token) | auth status | FAIL | 1 | exit 1 (want 0): forgejo.localhost   X Failed to log in to forgejo.localhost using token (default)   - Active account: true  |
| 91 | auth login (second account) | auth login | FAIL | 1 | exit 1 (want 0): error validating token: HTTP 404: 404 Not Found (https://forgejo.localhost/api/v3/)  |
| 92 | auth status (two accounts) | auth status | FAIL | 1 | exit 1 (want 0): forgejo.localhost   X Failed to log in to forgejo.localhost using token (default)   - Active account: true  |
| 93 | auth switch | auth switch | FAIL | 1 | exit 1 (want 0): not logged in to forgejo.localhost account alice-e94ba7  |
| 94 | auth token | auth token | FAIL | 1 | exit 1 (want 0): no oauth token found for forgejo.localhost  |
| 95 | auth token --user | auth token | FAIL | 1 | exit 1 (want 0): no oauth token found for forgejo.localhost account bob-e94ba7  |
| 96 | auth setup-git | auth setup-git | PASS | 0 |  |
| 97 | auth setup-git result | auth setup-git | PASS | 0 |  |
| 98 | auth refresh (device flow) | auth refresh | FAIL | 143 | exit 143 (want 0): /Users/vs/Workspace/GitHubClone/.claude/worktrees/agent-a0df397a2e1901171/tests/gh/run.sh: line 359: 96657 Terminated: 15          gh2 auth refresh --hostname " |
| 99 | auth refresh result | auth refresh | FAIL | 1 | exit 1 (want 0): forgejo.localhost   X Failed to log in to forgejo.localhost using token (default)   - Active account: true  |
| 100 | auth logout | auth logout | FAIL | 1 | exit 1 (want 0): not logged in to forgejo.localhost account bob-e94ba7  |
| 101 | auth logout result | auth logout | FAIL | 1 | exit 1 (want 0): forgejo.localhost   X Failed to log in to forgejo.localhost using token (default)   - Active account: true  |
| 102 | browse -n (repo) | browse | FAIL | 1 | exit 1 (want 0): https://forgejo.localhost/alice-e94ba7/demo doesn't exist  |
| 103 | browse -n (issue) | browse | FAIL | 1 | exit 1 (want 0): https://forgejo.localhost/alice-e94ba7/demo/issues/1 doesn't exist  |
| 104 | browse -n (file) | browse | FAIL | 1 | exit 1 (want 0): error determining the default branch: HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 105 | browse -n --settings | browse | FAIL | 1 | exit 1 (want 0): https://forgejo.localhost/alice-e94ba7/demo/settings doesn't exist  |
| 106 | browse -n --releases | browse | FAIL | 1 | exit 1 (want 0): https://forgejo.localhost/alice-e94ba7/demo/releases doesn't exist  |
| 107 | browse -n --commit | browse | FAIL | 1 | exit 1 (want 0): non-200 OK status code: 405 Method Not Allowed body: ""  |
| 108 | repo archive | repo archive | FAIL | 1 | exit 1 (want 0): HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 109 | repo archive result | repo archive | FAIL | 1 | exit 1 (want 0): HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 110 | repo unarchive | repo unarchive | FAIL | 1 | exit 1 (want 0): HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 111 | repo unarchive result | repo unarchive | FAIL | 1 | exit 1 (want 0): HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 112 | repo rename | repo rename | FAIL | 1 | exit 1 (want 0): HTTP 404: 404 Not Found (https://forgejo.localhost/api/v3/repos/alice-e94ba7/readme)  |
| 113 | repo rename result | repo rename | FAIL | 1 | exit 1 (want 0): HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 114 | repo edit --add-topic | repo edit | FAIL | 1 | exit 1 (want 0): HTTP 404: 404 Not Found (https://forgejo.localhost/api/v3/repos/alice-e94ba7/demo/topics)  |
| 115 | repo view --json (topics, more fields) | repo view | FAIL | 1 | exit 1 (want 0): HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 116 | repo list --json --jq | repo list | FAIL | 1 | exit 1 (want 0): non-200 OK status code: 405 Method Not Allowed body: ""  |
| 117 | repo list --topic | repo list | FAIL | 1 | exit 1 (want 0): non-200 OK status code: 405 Method Not Allowed body: ""  |
| 118 | repo set-default | repo set-default | FAIL | 1 | exit 1 (want 0): /Users/vs/Workspace/GitHubClone/.claude/worktrees/agent-a0df397a2e1901171/tests/gh/run.sh: line 164: cd: /Users/vs/Workspace/GitHubClone/.claude/worktrees/agent |
| 119 | repo set-default --view | repo set-default | FAIL | 1 | exit 1 (want 0): /Users/vs/Workspace/GitHubClone/.claude/worktrees/agent-a0df397a2e1901171/tests/gh/run.sh: line 164: cd: /Users/vs/Workspace/GitHubClone/.claude/worktrees/agent |
| 120 | repo sync (local) | repo sync | FAIL | 1 | exit 1 (want 0): /Users/vs/Workspace/GitHubClone/.claude/worktrees/agent-a0df397a2e1901171/tests/gh/run.sh: line 164: cd: /Users/vs/Workspace/GitHubClone/.claude/worktrees/agent |
| 121 | repo sync (local) result | repo sync | FAIL | 1 | exit 1 (want 0): /Users/vs/Workspace/GitHubClone/.claude/worktrees/agent-a0df397a2e1901171/tests/gh/run.sh: line 164: cd: /Users/vs/Workspace/GitHubClone/.claude/worktrees/agent |
| 122 | repo sync (fork from parent) | repo sync | FAIL | 1 | exit 1 (want 0): HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 123 | repo sync (fork) result | repo sync | FAIL | 1 | exit 1 (want 0): invalid character 'N' looking for beginning of value  |
| 124 | repo deploy-key add | repo deploy-key add | FAIL | 1 | exit 1 (want 0): HTTP 404: 404 Not Found (https://forgejo.localhost/api/v3/repos/alice-e94ba7/demo/keys)  |
| 125 | repo deploy-key list | repo deploy-key list | FAIL | 1 | exit 1 (want 0): HTTP 404: 404 Not Found (https://forgejo.localhost/api/v3/repos/alice-e94ba7/demo/keys?per_page=100)  |
| 126 | repo deploy-key list --json | repo deploy-key list | FAIL | 1 | exit 1 (want 0): HTTP 404: 404 Not Found (https://forgejo.localhost/api/v3/repos/alice-e94ba7/demo/keys?per_page=100)  |
| 127 | repo deploy-key delete | repo deploy-key delete | FAIL | 1 | exit 1 (want 0): HTTP 404: 404 Not Found (https://forgejo.localhost/api/v3/repos/alice-e94ba7/demo/keys/)  |
| 128 | repo autolink create | repo autolink create | FAIL | 1 | exit 1 (want 0): error creating autolink: HTTP 404: Must have admin rights to Repository. (https://forgejo.localhost/api/v3/repos/alice-e94ba7/demo/autolinks)  |
| 129 | repo autolink list | repo autolink list | FAIL | 1 | exit 1 (want 0): error getting autolinks: HTTP 404: Perhaps you are missing admin rights to the repository? (https://forgejo.localhost/api/v3/repos/alice-e94ba7/demo/autolinks)  |
| 130 | repo autolink list --json | repo autolink list | FAIL | 1 | exit 1 (want 0): error getting autolinks: HTTP 404: Perhaps you are missing admin rights to the repository? (https://forgejo.localhost/api/v3/repos/alice-e94ba7/demo/autolinks)  |
| 131 | repo autolink view | repo autolink view | FAIL | 1 | exit 1 (want 0): error viewing autolink: HTTP 404: Perhaps you are missing admin rights to the repository? (https://forgejo.localhost/api/v3/repos/alice-e94ba7/demo/autolinks/)  |
| 132 | repo autolink delete | repo autolink delete | FAIL | 1 | exit 1 (want 0): error deleting autolink: HTTP 404: Perhaps you are missing admin rights to the repository? (https://forgejo.localhost/api/v3/repos/alice-e94ba7/demo/autolinks/) |
| 133 | repo gitignore list | repo gitignore list | FAIL | 1 | exit 1 (want 0): HTTP 404: 404 Not Found (https://forgejo.localhost/api/v3/gitignore/templates)  |
| 134 | repo gitignore view | repo gitignore view | FAIL | 1 | exit 1 (want 0): 'Rust' is not a valid gitignore template. Run `gh repo gitignore list` for options  |
| 135 | repo license list | repo license list | FAIL | 1 | exit 1 (want 0): HTTP 404: 404 Not Found (https://forgejo.localhost/api/v3/licenses)  |
| 136 | repo license view | repo license view | FAIL | 1 | exit 1 (want 0): 'mit' is not a valid license name or SPDX ID. Run `gh repo license list` to see available commonly used licenses. For even more licenses, visit https://chooseal |
| 137 | repo read-file | repo read-file | FAIL | 1 | exit 1 (want 0): HTTP 404: 404 Not Found (https://forgejo.localhost/api/v3/repos/alice-e94ba7/demo/contents/README.md)  |
| 138 | repo read-file --ref --json | repo read-file | FAIL | 1 | exit 1 (want 0): HTTP 404: 404 Not Found (https://forgejo.localhost/api/v3/repos/alice-e94ba7/demo/contents/feature4.txt?ref=feature4)  |
| 139 | repo read-dir | repo read-dir | FAIL | 1 | exit 1 (want 0): non-200 OK status code: 405 Method Not Allowed body: ""  |
| 140 | repo read-dir PATH --json | repo read-dir | FAIL | 1 | exit 1 (want 0): non-200 OK status code: 405 Method Not Allowed body: ""  |
| 141 | issue create (for transfer) | issue create | FAIL | 1 | exit 1 (want 0): non-200 OK status code: 405 Method Not Allowed body: ""  |
| 142 | issue transfer | issue transfer | FAIL | 1 | exit 1 (want 0): invalid issue format: ""  |
| 143 | issue transfer result | issue transfer | FAIL | 1 | exit 1 (want 0): HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 144 | issue transfer (gone from source) | issue transfer | FAIL | 1 | output did not match /Could not resolve\|not found/: invalid issue format: ""  |
| 145 | issue pin | issue pin | FAIL | 1 | exit 1 (want 0): HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 146 | issue pin result | issue pin | FAIL | 1 | exit 1 (want 0): HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 147 | issue unpin | issue unpin | FAIL | 1 | exit 1 (want 0): HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 148 | issue unpin result | issue unpin | FAIL | 1 | exit 1 (want 0): HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 149 | issue lock | issue lock | FAIL | 1 | exit 1 (want 0): HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 150 | issue lock result | issue lock | FAIL | 1 | exit 1 (want 0): invalid character 'N' looking for beginning of value  |
| 151 | issue unlock | issue unlock | FAIL | 1 | exit 1 (want 0): HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 152 | issue unlock result | issue unlock | FAIL | 1 | exit 1 (want 0): invalid character 'N' looking for beginning of value  |
| 153 | issue develop | issue develop | FAIL | 1 | exit 1 (want 0): HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 154 | issue develop --list | issue develop | FAIL | 1 | exit 1 (want 0): HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 155 | issue develop result | issue develop | FAIL | 1 | exit 1 (want 0): invalid character 'N' looking for beginning of value  |
| 156 | issue comment --edit-last | issue comment | FAIL | 1 | exit 1 (want 0): HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 157 | issue status --json | issue status | FAIL | 1 | exit 1 (want 0): non-200 OK status code: 405 Method Not Allowed body: ""  |
| 158 | issue list --json --jq | issue list | FAIL | 1 | exit 1 (want 0): HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 159 | issue create --milestone | issue create | FAIL | 1 | exit 1 (want 0): non-200 OK status code: 405 Method Not Allowed body: ""  |
| 160 | issue list --milestone | issue list | FAIL | 1 | exit 1 (want 0): HTTP 404: 404 Not Found (https://forgejo.localhost/api/v3/meta)  |
| 161 | issue edit (two issues) | issue edit | FAIL | 1 | exit 1 (want 0): invalid issue format: ""  |
| 162 | issue close --json view | issue close | FAIL | 1 | exit 1 (want 0): HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 163 | issue reopen (again) | issue reopen | FAIL | 1 | exit 1 (want 0): HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 164 | pr lock | pr lock | FAIL | 1 | exit 1 (want 0): HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 165 | pr lock result | pr lock | FAIL | 1 | exit 1 (want 0): invalid character 'N' looking for beginning of value  |
| 166 | pr unlock | pr unlock | FAIL | 1 | exit 1 (want 0): HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 167 | pr unlock result | pr unlock | FAIL | 1 | exit 1 (want 0): invalid character 'N' looking for beginning of value  |
| 168 | pr update-branch | pr update-branch | FAIL | 1 | exit 1 (want 0): HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 169 | pr update-branch result | pr update-branch | FAIL | 1 | exit 1 (want 0): invalid character 'N' looking for beginning of value  |
| 170 | pr create (behind base) | pr create | FAIL | 1 | exit 1 (want 0): HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 171 | pr update-branch --rebase | pr update-branch | FAIL | 1 | exit 1 (want 0): HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 172 | pr update-branch --rebase result | pr update-branch | FAIL | 1 | exit 1 (want 0): invalid character 'N' looking for beginning of value  |
| 173 | pr revert | pr revert | FAIL | 1 | exit 1 (want 0): HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 174 | pr revert result | pr revert | FAIL | 1 | exit 1 (want 0): HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 175 | pr revert diff | pr revert | FAIL | 1 | exit 1 (want 0): HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 176 | pr comment --edit-last | pr comment | FAIL | 1 | exit 1 (want 0): HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 177 | pr view --comments | pr view | FAIL | 1 | exit 1 (want 0): HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 178 | pr status --json | pr status | FAIL | 1 | exit 1 (want 0): non-200 OK status code: 405 Method Not Allowed body: ""  |
| 179 | pr checks --json | pr checks | FAIL | 1 | exit 1 (want 0): HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 180 | pr diff --patch | pr diff | FAIL | 1 | exit 1 (want 0): HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 181 | pr list --json --jq | pr list | FAIL | 1 | exit 1 (want 0): HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 182 | pr list --label | pr list | FAIL | 1 | exit 1 (want 0): HTTP 404: 404 Not Found (https://forgejo.localhost/api/v3/meta)  |
| 183 | pr edit --add-reviewer | pr edit | FAIL | 1 | exit 1 (want 0): non-200 OK status code: 405 Method Not Allowed body: ""  |
| 184 | pr review --request-changes -> approve | pr review | FAIL | 1 | exit 1 (want 0): HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 185 | label edit | label edit | FAIL | 1 | exit 1 (want 0): HTTP 404: 404 Not Found (https://forgejo.localhost/api/v3/repos/alice-e94ba7/demo/labels/triage)  |
| 186 | label edit result | label edit | FAIL | 1 | exit 1 (want 0): HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 187 | label clone | label clone | FAIL | 1 | exit 1 (want 0): HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 188 | label clone result | label clone | FAIL | 1 | exit 1 (want 0): HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 189 | label delete | label delete | FAIL | 1 | exit 1 (want 0): HTTP 404: 404 Not Found (https://forgejo.localhost/api/v3/repos/alice-e94ba7/demo/labels/gql-label)  |
| 190 | label delete result | label delete | FAIL | 1 | output did not match /Not Found/: Not found. gh: HTTP 404  |
| 191 | release create (with asset) | release create | FAIL | 1 | exit 1 (want 0): HTTP 404: 404 Not Found (https://forgejo.localhost/api/v3/repos/alice-e94ba7/demo/releases)  |
| 192 | release create --draft --generate-notes | release create | FAIL | 1 | exit 1 (want 0): HTTP 404: 404 Not Found (https://forgejo.localhost/api/v3/repos/alice-e94ba7/demo/releases)  |
| 193 | release upload | release upload | FAIL | 1 | exit 1 (want 0): release not found  |
| 194 | release view --json | release view | FAIL | 1 | exit 1 (want 0): release not found  |
| 195 | release list --json | release list | FAIL | 1 | exit 1 (want 0): non-200 OK status code: 405 Method Not Allowed body: ""  |
| 196 | release download (pattern) | release download | FAIL | 1 | exit 1 (want 0): release not found  |
| 197 | release download result | release download | FAIL | 1 | exit 1 (want 0): cat: /Users/vs/Workspace/GitHubClone/.claude/worktrees/agent-a0df397a2e1901171/data/bench/gh-forgejo/dl/asset.txt: No such file or directory  |
| 198 | release download --archive | release download | FAIL | 1 | exit 1 (want 0): release not found  |
| 199 | release download --archive result | release download | FAIL | 1 | exit 1 (want 0): tar: /Users/vs/Workspace/GitHubClone/.claude/worktrees/agent-a0df397a2e1901171/data/bench/gh-forgejo/dl/demo-v1.1.tar.gz: m: No such file or directory  |
| 200 | release edit | release edit | FAIL | 1 | exit 1 (want 0): release not found  |
| 201 | release edit result | release edit | FAIL | 1 | exit 1 (want 0): release not found  |
| 202 | release delete-asset | release delete-asset | FAIL | 1 | exit 1 (want 0): release not found  |
| 203 | release delete-asset result | release delete-asset | FAIL | 1 | exit 1 (want 0): release not found  |
| 204 | release delete | release delete | FAIL | 1 | exit 1 (want 0): release not found  |
| 205 | release delete result | release delete | PASS | 1 |  |
| 206 | gist create | gist create | FAIL | 1 | exit 1 (want 0): - Creating gist hello.txt X Failed to create gist: HTTP 404: 404 Not Found (https://forgejo.localhost/api/v3/gists)  |
| 207 | gist create (stdin) | gist create | FAIL | 1 | exit 1 (want 0): - Creating gist piped.txt X Failed to create gist: HTTP 404: 404 Not Found (https://forgejo.localhost/api/v3/gists)  |
| 208 | gist list | gist list | FAIL | 1 | exit 1 (want 0): non-200 OK status code: 405 Method Not Allowed body: ""  |
| 209 | gist list --public | gist list | FAIL | 1 | exit 1 (want 0): non-200 OK status code: 405 Method Not Allowed body: ""  |
| 210 | gist view | gist view | FAIL | 1 | exit 1 (want 0): gist ID or URL required when not running interactively View the given gist or select from recent gists. USAGE  |
| 211 | gist view --files | gist view | FAIL | 1 | exit 1 (want 0): gist ID or URL required when not running interactively View the given gist or select from recent gists. USAGE  |
| 212 | gist edit --add | gist edit | FAIL | 1 | exit 1 (want 0): gist ID or URL required when not running interactively Edit one of your gists USAGE  |
| 213 | gist edit FILE (replace content) | gist edit | FAIL | 1 | exit 1 (want 0): gist ID or URL required when not running interactively Edit one of your gists USAGE  |
| 214 | gist edit result | gist edit | FAIL | 1 | exit 1 (want 0): gist ID or URL required when not running interactively View the given gist or select from recent gists. USAGE  |
| 215 | gist edit --desc | gist edit | FAIL | 1 | exit 1 (want 0): gist ID or URL required when not running interactively Edit one of your gists USAGE  |
| 216 | gist rename | gist rename | FAIL | 1 | exit 1 (want 0): gist not found:   |
| 217 | gist rename result | gist rename | FAIL | 1 | exit 1 (want 0): gist ID or URL required when not running interactively View the given gist or select from recent gists. USAGE  |
| 218 | gist clone | gist clone | FAIL | 1 | exit 1 (want 0): Cloning into '/Users/vs/Workspace/GitHubClone/.claude/worktrees/agent-a0df397a2e1901171/data/bench/gh-forgejo/work/gistclone'... remote: Not found. fatal: repos |
| 219 | gist clone result | gist clone | FAIL | 1 | exit 1 (want 0): ls: /Users/vs/Workspace/GitHubClone/.claude/worktrees/agent-a0df397a2e1901171/data/bench/gh-forgejo/work/gistclone: No such file or directory  |
| 220 | gist delete | gist delete | FAIL | 1 | exit 1 (want 0): non-200 OK status code: 405 Method Not Allowed body: ""  |
| 221 | gist delete result | gist delete | FAIL | 1 | output did not match /404\|not found/: gist ID or URL required when not running interactively View the given gist or select from recent gists. USAGE  |
| 222 | ssh-key add | ssh-key add | FAIL | 1 | exit 1 (want 0): HTTP 404: 404 Not Found (https://forgejo.localhost/api/v3/user/keys?per_page=100)  |
| 223 | ssh-key add --type signing | ssh-key add | FAIL | 1 | exit 1 (want 0): HTTP 404: 404 Not Found (https://forgejo.localhost/api/v3/user/ssh_signing_keys?per_page=100)  |
| 224 | ssh-key list | ssh-key list | FAIL | 1 | exit 1 (want 0): warning:  HTTP 404: 404 Not Found (https://forgejo.localhost/api/v3/user/keys?per_page=100) warning:  HTTP 404: 404 Not Found (https://forgejo.localhost/api/v3/ |
| 225 | ssh-key delete | ssh-key delete | FAIL | 1 | exit 1 (want 0): HTTP 404: 404 Not Found (https://forgejo.localhost/api/v3/user/keys/)  |
| 226 | ssh-key delete result | ssh-key delete | FAIL | 1 | exit 1 (want 0): invalid character 'N' looking for beginning of value  |
| 227 | gpg-key add | gpg-key add | FAIL | 1 | exit 1 (want 0): Error: insufficient OAuth scopes to list GPG keys Run the following to grant scopes: gh auth refresh -s write:gpg_key  |
| 228 | gpg-key list | gpg-key list | FAIL | 1 | exit 1 (want 0): Error: insufficient OAuth scopes to list GPG keys Run the following to grant scopes: gh auth refresh -s read:gpg_key  |
| 229 | gpg-key delete | gpg-key delete | FAIL | 1 | exit 1 (want 0): HTTP 404: 404 Not Found (https://forgejo.localhost/api/v3/user/gpg_keys?per_page=100)  |
| 230 | gpg-key delete result | gpg-key delete | FAIL | 1 | exit 1 (want 0): invalid character 'N' looking for beginning of value  |
| 231 | org list | org list | FAIL | 1 | exit 1 (want 0): non-200 OK status code: 405 Method Not Allowed body: ""  |
| 232 | search repos | search repos | FAIL | 1 | exit 1 (want 0): HTTP 404: 404 Not Found (https://forgejo.localhost/api/v3/search/repositories?page=1&per_page=30&q=demo+user%3Aalice-e94ba7)  |
| 233 | search repos --json | search repos | FAIL | 1 | exit 1 (want 0): HTTP 404: 404 Not Found (https://forgejo.localhost/api/v3/search/repositories?page=1&per_page=30&q=demo+user%3Aalice-e94ba7)  |
| 234 | search issues | search issues | FAIL | 1 | exit 1 (want 0): HTTP 404: 404 Not Found (https://forgejo.localhost/api/v3/meta)  |
| 235 | search issues --json | search issues | FAIL | 1 | exit 1 (want 0): HTTP 404: 404 Not Found (https://forgejo.localhost/api/v3/meta)  |
| 236 | search prs | search prs | FAIL | 1 | exit 1 (want 0): HTTP 404: 404 Not Found (https://forgejo.localhost/api/v3/meta)  |
| 237 | search prs --json --state | search prs | FAIL | 1 | exit 1 (want 0): HTTP 404: 404 Not Found (https://forgejo.localhost/api/v3/meta)  |
| 238 | search commits | search commits | FAIL | 1 | exit 1 (want 0): HTTP 404: 404 Not Found (https://forgejo.localhost/api/v3/search/commits?page=1&per_page=30&q=feature+repo%3Aalice-e94ba7%2Fdemo)  |
| 239 | search commits --json | search commits | FAIL | 1 | exit 1 (want 0): HTTP 404: 404 Not Found (https://forgejo.localhost/api/v3/search/commits?page=1&per_page=30&q=feature+repo%3Aalice-e94ba7%2Fdemo)  |
| 240 | search code | search code | FAIL | 1 | exit 1 (want 0): HTTP 404: 404 Not Found (https://forgejo.localhost/api/v3/search/code?page=1&per_page=30&q=demo+repo%3Aalice-e94ba7%2Fdemo)  |
| 241 | search code --json | search code | FAIL | 1 | exit 1 (want 0): HTTP 404: 404 Not Found (https://forgejo.localhost/api/v3/search/code?page=1&per_page=30&q=demo+repo%3Aalice-e94ba7%2Fdemo)  |
| 242 | status | status | FAIL | 1 | exit 1 (want 0): failed to search: could not search for assignments: HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 243 | status --org | status | FAIL | 1 | exit 1 (want 0): could not load events: failed to get current username: non-200 OK status code: 405 Method Not Allowed body: ""  |
| 244 | api -X PATCH | api | FAIL | 1 | exit 1 (want 0): invalid character 'N' looking for beginning of value  |
| 245 | api --include | api | FAIL | 1 | exit 1 (want 0): HTTP/1.1 404 Not Found Cache-Control: max-age=0, private, must-revalidate Content-Length: 11  |
| 246 | api --template | api | FAIL | 1 | exit 1 (want 0): invalid character 'N' looking for beginning of value  |
| 247 | api --input | api | FAIL | 1 | exit 1 (want 0): Not found. gh: HTTP 404  |
| 248 | api graphql -F variables | api | FAIL | 1 | exit 1 (want 0): gh: HTTP 405  |
| 249 | api 404 | api | FAIL | 1 | output did not match /Not Found/: Not found. gh: HTTP 404  |
| 250 | ruleset list | ruleset list | FAIL | 1 | exit 1 (want 0): HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 251 | ruleset view | ruleset view | FAIL | 1 | exit 1 (want 0): invalid value for ruleset ID:  is not an integer View information about a GitHub ruleset. If no ID is provided, an interactive prompt will be used to choose  |
| 252 | ruleset check | ruleset check | FAIL | 1 | exit 1 (want 0): GET repos/alice-e94ba7/demo/rules/branches/main failed: HTTP 404: 404 Not Found (https://forgejo.localhost/api/v3/repos/alice-e94ba7/demo/rules/branches/main)  |
| 253 | workflow list | workflow list | FAIL | 1 | exit 1 (want 0): could not get workflows: HTTP 404: 404 Not Found (https://forgejo.localhost/api/v3/repos/alice-e94ba7/demo/actions/workflows?page=1&per_page=50)  |
| 254 | workflow list --json | workflow list | FAIL | 1 | exit 1 (want 0): could not get workflows: HTTP 404: 404 Not Found (https://forgejo.localhost/api/v3/repos/alice-e94ba7/demo/actions/workflows?page=1&per_page=50)  |
| 255 | workflow view | workflow view | FAIL | 1 | exit 1 (want 0): HTTP 404: workflow ci.yml not found on the default branch (https://forgejo.localhost/api/v3/repos/alice-e94ba7/demo/actions/workflows/ci.yml)  |
| 256 | workflow disable | workflow disable | FAIL | 1 | exit 1 (want 0): HTTP 404: workflow ci.yml not found on the default branch (https://forgejo.localhost/api/v3/repos/alice-e94ba7/demo/actions/workflows/ci.yml)  |
| 257 | workflow disable result | workflow disable | FAIL | 1 | exit 1 (want 0): could not get workflows: HTTP 404: 404 Not Found (https://forgejo.localhost/api/v3/repos/alice-e94ba7/demo/actions/workflows?page=1&per_page=50)  |
| 258 | workflow enable | workflow enable | FAIL | 1 | exit 1 (want 0): HTTP 404: workflow ci.yml not found on the default branch (https://forgejo.localhost/api/v3/repos/alice-e94ba7/demo/actions/workflows/ci.yml)  |
| 259 | workflow enable result | workflow enable | FAIL | 1 | exit 1 (want 0): could not get workflows: HTTP 404: 404 Not Found (https://forgejo.localhost/api/v3/repos/alice-e94ba7/demo/actions/workflows?page=1&per_page=50)  |
| 260 | workflow run | workflow run | FAIL | 1 | exit 1 (want 0): HTTP 404: workflow ci.yml not found on the default branch (https://forgejo.localhost/api/v3/repos/alice-e94ba7/demo/actions/workflows/ci.yml)  |
| 261 | run list | run list | FAIL | 1 | exit 1 (want 0): failed to get runs: HTTP 404: 404 Not Found (https://forgejo.localhost/api/v3/repos/alice-e94ba7/demo/actions/runs?exclude_pull_requests=true&per_page=20)  |
| 262 | run list --json | run list | FAIL | 1 | exit 1 (want 0): failed to get runs: HTTP 404: 404 Not Found (https://forgejo.localhost/api/v3/repos/alice-e94ba7/demo/actions/runs?exclude_pull_requests=true&per_page=20)  |
| 263 | run view | run view | FAIL | 1 | exit 1 (want 0): failed to get run: HTTP 404: 404 Not Found (https://forgejo.localhost/api/v3/repos/alice-e94ba7/demo/actions/runs/?exclude_pull_requests=true)  |
| 264 | run view --json | run view | FAIL | 1 | exit 1 (want 0): failed to get run: HTTP 404: 404 Not Found (https://forgejo.localhost/api/v3/repos/alice-e94ba7/demo/actions/runs/?exclude_pull_requests=true)  |
| 265 | run cancel | run cancel | FAIL | 1 | exit 1 (want 0): Could not find any workflow run with ID   |
| 266 | run watch | run watch | FAIL | 1 | exit 1 (want 0): failed to get run: HTTP 404: 404 Not Found (https://forgejo.localhost/api/v3/repos/alice-e94ba7/demo/actions/runs/?exclude_pull_requests=true)  |
| 267 | run rerun | run rerun | FAIL | 1 | exit 1 (want 0): failed to get run: HTTP 404: 404 Not Found (https://forgejo.localhost/api/v3/repos/alice-e94ba7/demo/actions/runs/?exclude_pull_requests=true)  |
| 268 | run download | run download | FAIL | 1 | output did not match /no valid artifacts/: error fetching artifacts: HTTP 404: 404 Not Found (https://forgejo.localhost/api/v3/repos/alice-e94ba7/demo/actions/artifacts?per_page=100)  |
| 269 | run delete | run delete | FAIL | 1 | exit 1 (want 0): could not find any workflow run with ID   |
| 270 | cache list | cache list | FAIL | 1 | exit 1 (want 0): X Failed to get caches: HTTP 404: 404 Not Found (https://forgejo.localhost/api/v3/repos/alice-e94ba7/demo/actions/caches?direction=desc&per_page=30&sort=last_ac |
| 271 | cache list --json | cache list | FAIL | 1 | exit 1 (want 0): X Failed to get caches: HTTP 404: 404 Not Found (https://forgejo.localhost/api/v3/repos/alice-e94ba7/demo/actions/caches?direction=desc&per_page=30&sort=last_ac |
| 272 | cache delete --all | cache delete | FAIL | 1 | exit 1 (want 0): HTTP 404: 404 Not Found (https://forgejo.localhost/api/v3/repos/alice-e94ba7/demo/actions/caches?per_page=100)  |
| 273 | secret set | secret set | FAIL | 1 | exit 1 (want 0): failed to fetch public key: HTTP 404: 404 Not Found (https://forgejo.localhost/api/v3/repos/alice-e94ba7/demo/actions/secrets/public-key)  |
| 274 | secret list | secret list | FAIL | 1 | exit 1 (want 0): failed to get secrets: HTTP 404: 404 Not Found (https://forgejo.localhost/api/v3/repos/alice-e94ba7/demo/actions/secrets?per_page=100)  |
| 275 | secret list --json | secret list | FAIL | 1 | exit 1 (want 0): failed to get secrets: HTTP 404: 404 Not Found (https://forgejo.localhost/api/v3/repos/alice-e94ba7/demo/actions/secrets?per_page=100)  |
| 276 | secret delete | secret delete | FAIL | 1 | exit 1 (want 0): failed to delete secret API_KEY: HTTP 404: 404 Not Found (https://forgejo.localhost/api/v3/repos/alice-e94ba7/demo/actions/secrets/API_KEY)  |
| 277 | secret delete result | secret delete | FAIL | 1 | exit 1 (want 0): failed to get secrets: HTTP 404: 404 Not Found (https://forgejo.localhost/api/v3/repos/alice-e94ba7/demo/actions/secrets?per_page=100)  |
| 278 | variable set | variable set | FAIL | 1 | exit 1 (want 0): failed to set variable "GREETING": HTTP 404: 404 Not Found (https://forgejo.localhost/api/v3/repos/alice-e94ba7/demo/actions/variables)  |
| 279 | variable list | variable list | FAIL | 1 | exit 1 (want 0): failed to get variables: HTTP 404: 404 Not Found (https://forgejo.localhost/api/v3/repos/alice-e94ba7/demo/actions/variables?per_page=100)  |
| 280 | variable list --json | variable list | FAIL | 1 | exit 1 (want 0): failed to get variables: HTTP 404: 404 Not Found (https://forgejo.localhost/api/v3/repos/alice-e94ba7/demo/actions/variables?per_page=100)  |
| 281 | variable get | variable get | FAIL | 1 | exit 1 (want 0): variable GREETING was not found  |
| 282 | variable set (update) | variable set | FAIL | 1 | exit 1 (want 0): failed to set variable "GREETING": HTTP 404: 404 Not Found (https://forgejo.localhost/api/v3/repos/alice-e94ba7/demo/actions/variables)  |
| 283 | variable set result | variable set | FAIL | 1 | exit 1 (want 0): variable GREETING was not found  |
| 284 | variable delete | variable delete | FAIL | 1 | exit 1 (want 0): failed to delete variable GREETING: HTTP 404: 404 Not Found (https://forgejo.localhost/api/v3/repos/alice-e94ba7/demo/actions/variables/GREETING)  |
| 285 | alias set | alias set | PASS | 0 |  |
| 286 | alias set (use) | alias set | FAIL | 1 | exit 1 (want 0): HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 287 | alias list | alias list | PASS | 0 |  |
| 288 | alias import | alias import | PASS | 0 |  |
| 289 | alias import (use) | alias import | FAIL | 1 | exit 1 (want 0): HTTP 405: 405 Method Not Allowed (https://forgejo.localhost/api/graphql)  |
| 290 | alias delete | alias delete | PASS | 0 |  |
| 291 | config set | config set | PASS | 0 |  |
| 292 | config get | config get | PASS | 0 |  |
| 293 | config list | config list | PASS | 0 |  |
| 294 | config clear-cache | config clear-cache | PASS | 0 |  |
| 295 | completion | completion | PASS | 0 |  |
| 296 | licenses | licenses | PASS | 0 |  |
| 297 | extension install | extension install | FAIL | 1 | exit 1 (want 0): X Could not find extension 'forgejo.localhost/alice-e94ba7/gh-hello' on host forgejo.localhost  |
| 298 | extension list | extension list | FAIL | 0 | output did not match /hello/:  |
| 299 | extension exec | extension exec | FAIL | 1 | exit 1 (want 0): extension "hello" not found  |
| 300 | extension upgrade | extension upgrade | FAIL | 1 | exit 1 (want 0): X Failed upgrading extension hello: no extensions installed  |
| 301 | extension upgrade result | extension upgrade | FAIL | 1 | exit 1 (want 0): unknown command "hello" for "gh" Work seamlessly with GitHub from the command line. USAGE  |
| 302 | extension search | extension search | FAIL | 1 | exit 1 (want 0): HTTP 404: 404 Not Found (https://forgejo.localhost/api/v3/search/repositories?page=1&per_page=30&q=hello+topic%3Agh-extension)  |
| 303 | extension remove | extension remove | FAIL | 1 | exit 1 (want 0): no extension found: "/Users/vs/Workspace/GitHubClone/.claude/worktrees/agent-a0df397a2e1901171/data/bench/gh-forgejo/xdg-data/gh/extensions/gh-hello"  |
| 304 | extension create | extension create | PASS | 0 |  |
| 305 | extension create result | extension create | PASS | 0 |  |
| 306 | skill list | skill list | PASS | 0 |  |
| 307 | repo edit --enable-discussions | repo edit | FAIL | 1 | exit 1 (want 0): HTTP 404: 404 Not Found (https://forgejo.localhost/api/v3/repos/alice-e94ba7/demo)  |
| 308 | discussion create | discussion create | FAIL | 1 | exit 1 (want 0): non-200 OK status code: 405 Method Not Allowed body: ""  |
| 309 | discussion create (Q&A) | discussion create | FAIL | 1 | exit 1 (want 0): non-200 OK status code: 405 Method Not Allowed body: ""  |
| 310 | discussion list | discussion list | FAIL | 1 | exit 1 (want 0): non-200 OK status code: 405 Method Not Allowed body: ""  |
| 311 | discussion list --json | discussion list | FAIL | 1 | exit 1 (want 0): non-200 OK status code: 405 Method Not Allowed body: ""  |
| 312 | discussion list --category | discussion list | FAIL | 1 | exit 1 (want 0): non-200 OK status code: 405 Method Not Allowed body: ""  |
| 313 | discussion view | discussion view | FAIL | 1 | exit 1 (want 0): invalid argument: "" (expected a discussion number, URL, or comment ID) Display the title, body, and other information about a discussion. To see the comments o |
| 314 | discussion comment | discussion comment | FAIL | 1 | exit 1 (want 0): invalid argument: "" (expected a discussion number, URL, or comment ID)  |
| 315 | discussion view --comments | discussion view | FAIL | 1 | exit 1 (want 0): invalid argument: "" (expected a discussion number, URL, or comment ID) Display the title, body, and other information about a discussion. To see the comments o |
| 316 | discussion edit | discussion edit | FAIL | 1 | exit 1 (want 0): invalid discussion argument: "" Edit a GitHub Discussion. Without flags, the command runs interactively when connected to a terminal.  |
| 317 | discussion edit result | discussion edit | FAIL | 1 | exit 1 (want 0): invalid argument: "" (expected a discussion number, URL, or comment ID) Display the title, body, and other information about a discussion. To see the comments o |
| 318 | project create | project create | FAIL | 1 | exit 1 (want 0): unknown owner type  |
| 319 | project list | project list | FAIL | 1 | exit 1 (want 0): unknown owner type  |
| 320 | project list --format json | project list | FAIL | 1 | exit 1 (want 0): unknown owner type  |
| 321 | project view | project view | FAIL | 1 | exit 1 (want 0): unknown owner type  |
| 322 | project edit | project edit | FAIL | 1 | exit 1 (want 0): unknown owner type  |
| 323 | project field-create | project field-create | FAIL | 1 | exit 1 (want 0): unknown owner type  |
| 324 | project field-create (text) | project field-create | FAIL | 1 | exit 1 (want 0): unknown owner type  |
| 325 | project field-list | project field-list | FAIL | 1 | exit 1 (want 0): unknown owner type  |
| 326 | project item-create | project item-create | FAIL | 1 | exit 1 (want 0): unknown owner type  |
| 327 | project item-add | project item-add | FAIL | 1 | exit 1 (want 0): unknown owner type  |
| 328 | project item-list | project item-list | FAIL | 1 | exit 1 (want 0): unknown owner type  |
| 329 | project item-edit (draft) | project item-edit | FAIL | 1 | exit 1 (want 0): specify the item to edit with `--id` or `--url` Edit a draft issue or a project item. The usual way to select the item and field is by name: pass the project  |
| 330 | project item-edit (field by name) | project item-edit | FAIL | 1 | exit 1 (want 0): unknown owner type  |
| 331 | project item-edit result | project item-edit | FAIL | 1 | exit 1 (want 0): unknown owner type  |
| 332 | project item-list --query | project item-list | FAIL | 1 | exit 1 (want 0): non-200 OK status code: 405 Method Not Allowed body: ""  |
| 333 | project item-archive | project item-archive | FAIL | 1 | exit 1 (want 0): unknown owner type  |
| 334 | project item-archive result | project item-archive | FAIL | 1 | exit 1 (want 0): gh: HTTP 405  |
| 335 | project item-delete | project item-delete | FAIL | 1 | exit 1 (want 0): unknown owner type  |
| 336 | project item-delete result | project item-delete | FAIL | 1 | exit 1 (want 0): unknown owner type  |
| 337 | project field-delete | project field-delete | FAIL | 1 | exit 1 (want 0): non-200 OK status code: 405 Method Not Allowed body: ""  |
| 338 | project field-delete result | project field-delete | PASS | 1 |  |
| 339 | project link | project link | FAIL | 1 | exit 1 (want 0): unknown owner type  |
| 340 | project link result | project link | FAIL | 1 | exit 1 (want 0): gh: HTTP 405  |
| 341 | project unlink | project unlink | FAIL | 1 | exit 1 (want 0): unknown owner type  |
| 342 | project unlink result | project unlink | FAIL | 1 | exit 1 (want 0): gh: HTTP 405  |
| 343 | project copy | project copy | FAIL | 1 | exit 1 (want 0): unknown owner type  |
| 344 | project mark-template | project mark-template | FAIL | 1 | exit 1 (want 0): unknown owner type  |
| 345 | project mark-template result | project mark-template | FAIL | 1 | exit 1 (want 0): gh: HTTP 405  |
| 346 | project close | project close | FAIL | 1 | exit 1 (want 0): unknown owner type  |
| 347 | project close --undo | project close | FAIL | 1 | exit 1 (want 0): unknown owner type  |
| 348 | project delete | project delete | FAIL | 1 | exit 1 (want 0): unknown owner type  |
| 349 | project delete result | project delete | FAIL | 1 | output did not match /Could not resolve to a ProjectV2/: unknown owner type  |

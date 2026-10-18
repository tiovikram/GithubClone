# gh compatibility report

gh: `gh version 2.102.0 (2026-09-30)`, host `forge.localhost` (GHES path, http_unix_socket). Generated 2026-10-18T08:56:50Z.
Denominator: see docs/gh-coverage.md (catalogue: tests/gh/commands.tsv).

**PASS 166/166 forge-relevant gh commands (100%)**

**166/197 of all gh commands (84%)**

**Cases: PASS 363/363 (100%)**

## Commands

| Command | Category | Result | Cases passing | Note |
|---------|----------|--------|---------------|------|
| `gh agent-task create` | b | n/a | 0/0 | Copilot coding agent: github.com-only service (api.githubcopilot.com) |
| `gh agent-task list` | b | n/a | 0/0 | Copilot coding agent: github.com-only service |
| `gh agent-task view` | b | n/a | 0/0 | Copilot coding agent: github.com-only service |
| `gh alias delete` | a | PASS | 1/1 | local-only |
| `gh alias import` | a | PASS | 2/2 | local-only |
| `gh alias list` | a | PASS | 1/1 | local-only |
| `gh alias set` | a | PASS | 2/2 | local-only |
| `gh api` | a | PASS | 13/13 |  |
| `gh attestation download` | b | n/a | 0/0 | Sigstore/GitHub artifact attestations service |
| `gh attestation trusted-root` | b | n/a | 0/0 | Sigstore TUF trusted root (github.com / public-good Sigstore) |
| `gh attestation verify` | b | n/a | 0/0 | Sigstore/GitHub artifact attestations service |
| `gh auth login` | a | PASS | 2/2 |  |
| `gh auth logout` | a | PASS | 2/2 | local-only |
| `gh auth refresh` | a | PASS | 2/2 | needs the OAuth device/web flow |
| `gh auth setup-git` | a | PASS | 2/2 | local-only |
| `gh auth status` | a | PASS | 3/3 |  |
| `gh auth switch` | a | PASS | 1/1 | local-only |
| `gh auth token` | a | PASS | 2/2 | local-only |
| `gh browse` | a | PASS | 6/6 | tested with --no-browser (prints the URL) |
| `gh cache delete` | a | PASS | 1/1 | Actions caches (forge has none) |
| `gh cache list` | a | PASS | 2/2 | Actions caches (forge has none) |
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
| `gh discussion comment` | a | PASS | 1/1 | Discussions |
| `gh discussion create` | a | PASS | 2/2 | Discussions |
| `gh discussion edit` | a | PASS | 2/2 | Discussions |
| `gh discussion list` | a | PASS | 3/3 | Discussions |
| `gh discussion view` | a | PASS | 2/2 | Discussions |
| `gh extension browse` | b | n/a | 0/0 | full-screen TUI; needs a terminal, cannot run non-interactively |
| `gh extension create` | a | PASS | 2/2 | local-only (git) |
| `gh extension exec` | a | PASS | 1/1 | local-only |
| `gh extension install` | a | PASS | 1/1 | clones from the forge |
| `gh extension list` | a | PASS | 1/1 | local-only |
| `gh extension remove` | a | PASS | 1/1 | local-only |
| `gh extension search` | a | PASS | 1/1 | repository search (topic:gh-extension) |
| `gh extension upgrade` | a | PASS | 2/2 | fetches from the forge |
| `gh gist clone` | a | PASS | 2/2 |  |
| `gh gist create` | a | PASS | 2/2 |  |
| `gh gist delete` | a | PASS | 2/2 |  |
| `gh gist edit` | a | PASS | 4/4 |  |
| `gh gist list` | a | PASS | 2/2 |  |
| `gh gist rename` | a | PASS | 2/2 |  |
| `gh gist view` | a | PASS | 2/2 |  |
| `gh gpg-key add` | a | PASS | 1/1 |  |
| `gh gpg-key delete` | a | PASS | 2/2 |  |
| `gh gpg-key list` | a | PASS | 1/1 |  |
| `gh issue close` | a | PASS | 2/2 |  |
| `gh issue comment` | a | PASS | 2/2 |  |
| `gh issue create` | a | PASS | 5/5 |  |
| `gh issue delete` | a | PASS | 1/1 |  |
| `gh issue develop` | a | PASS | 3/3 | linked branches |
| `gh issue edit` | a | PASS | 3/3 |  |
| `gh issue list` | a | PASS | 8/8 |  |
| `gh issue lock` | a | PASS | 2/2 |  |
| `gh issue pin` | a | PASS | 2/2 |  |
| `gh issue reopen` | a | PASS | 2/2 |  |
| `gh issue status` | a | PASS | 2/2 |  |
| `gh issue transfer` | a | PASS | 3/3 |  |
| `gh issue unlock` | a | PASS | 2/2 |  |
| `gh issue unpin` | a | PASS | 2/2 |  |
| `gh issue view` | a | PASS | 5/5 |  |
| `gh label clone` | a | PASS | 2/2 |  |
| `gh label create` | a | PASS | 2/2 |  |
| `gh label delete` | a | PASS | 2/2 |  |
| `gh label edit` | a | PASS | 2/2 |  |
| `gh label list` | a | PASS | 2/2 |  |
| `gh licenses` | a | PASS | 1/1 | local-only |
| `gh org list` | a | PASS | 1/1 |  |
| `gh pr checkout` | a | PASS | 2/2 |  |
| `gh pr checks` | a | PASS | 2/2 |  |
| `gh pr close` | a | PASS | 1/1 |  |
| `gh pr comment` | a | PASS | 2/2 |  |
| `gh pr create` | a | PASS | 6/6 |  |
| `gh pr diff` | a | PASS | 3/3 |  |
| `gh pr edit` | a | PASS | 2/2 |  |
| `gh pr list` | a | PASS | 7/7 |  |
| `gh pr lock` | a | PASS | 2/2 |  |
| `gh pr merge` | a | PASS | 4/4 |  |
| `gh pr ready` | a | PASS | 3/3 |  |
| `gh pr reopen` | a | PASS | 1/1 |  |
| `gh pr revert` | a | PASS | 3/3 |  |
| `gh pr review` | a | PASS | 4/4 |  |
| `gh pr status` | a | PASS | 2/2 |  |
| `gh pr unlock` | a | PASS | 2/2 |  |
| `gh pr update-branch` | a | PASS | 4/4 |  |
| `gh pr view` | a | PASS | 8/8 |  |
| `gh preview prompter` | b | n/a | 0/0 | developer preview of gh's own TTY prompts; no host interaction, needs a terminal |
| `gh project close` | a | PASS | 2/2 | Projects (v2) |
| `gh project copy` | a | PASS | 1/1 | Projects (v2) |
| `gh project create` | a | PASS | 1/1 | Projects (v2) |
| `gh project delete` | a | PASS | 2/2 | Projects (v2) |
| `gh project edit` | a | PASS | 1/1 | Projects (v2) |
| `gh project field-create` | a | PASS | 2/2 | Projects (v2) |
| `gh project field-delete` | a | PASS | 2/2 | Projects (v2) |
| `gh project field-list` | a | PASS | 1/1 | Projects (v2) |
| `gh project item-add` | a | PASS | 1/1 | Projects (v2) |
| `gh project item-archive` | a | PASS | 2/2 | Projects (v2) |
| `gh project item-create` | a | PASS | 1/1 | Projects (v2) |
| `gh project item-delete` | a | PASS | 2/2 | Projects (v2) |
| `gh project item-edit` | a | PASS | 3/3 | Projects (v2) |
| `gh project item-list` | a | PASS | 2/2 | Projects (v2) |
| `gh project link` | a | PASS | 2/2 | Projects (v2) |
| `gh project list` | a | PASS | 2/2 | Projects (v2) |
| `gh project mark-template` | a | PASS | 2/2 | Projects (v2) |
| `gh project unlink` | a | PASS | 2/2 | Projects (v2) |
| `gh project view` | a | PASS | 1/1 | Projects (v2) |
| `gh release create` | a | PASS | 3/3 |  |
| `gh release delete` | a | PASS | 2/2 |  |
| `gh release delete-asset` | a | PASS | 2/2 |  |
| `gh release download` | a | PASS | 4/4 |  |
| `gh release edit` | a | PASS | 2/2 |  |
| `gh release list` | a | PASS | 2/2 |  |
| `gh release upload` | a | PASS | 1/1 |  |
| `gh release verify` | b | n/a | 0/0 | verifies Sigstore release attestations |
| `gh release verify-asset` | b | n/a | 0/0 | verifies Sigstore release attestations |
| `gh release view` | a | PASS | 2/2 |  |
| `gh repo archive` | a | PASS | 2/2 |  |
| `gh repo autolink create` | a | PASS | 1/1 |  |
| `gh repo autolink delete` | a | PASS | 1/1 |  |
| `gh repo autolink list` | a | PASS | 2/2 |  |
| `gh repo autolink view` | a | PASS | 1/1 |  |
| `gh repo clone` | a | PASS | 2/2 |  |
| `gh repo create` | a | PASS | 2/2 |  |
| `gh repo delete` | a | PASS | 1/1 |  |
| `gh repo deploy-key add` | a | PASS | 1/1 |  |
| `gh repo deploy-key delete` | a | PASS | 1/1 |  |
| `gh repo deploy-key list` | a | PASS | 2/2 |  |
| `gh repo edit` | a | PASS | 3/3 |  |
| `gh repo fork` | a | PASS | 1/1 |  |
| `gh repo gitignore list` | a | PASS | 1/1 |  |
| `gh repo gitignore view` | a | PASS | 1/1 |  |
| `gh repo license list` | a | PASS | 1/1 |  |
| `gh repo license view` | a | PASS | 1/1 |  |
| `gh repo list` | a | PASS | 5/5 |  |
| `gh repo read-dir` | a | PASS | 2/2 |  |
| `gh repo read-file` | a | PASS | 2/2 |  |
| `gh repo rename` | a | PASS | 2/2 |  |
| `gh repo set-default` | a | PASS | 2/2 | local-only (git config) |
| `gh repo sync` | a | PASS | 4/4 |  |
| `gh repo unarchive` | a | PASS | 2/2 |  |
| `gh repo view` | a | PASS | 6/6 |  |
| `gh ruleset check` | a | PASS | 1/1 |  |
| `gh ruleset list` | a | PASS | 1/1 |  |
| `gh ruleset view` | a | PASS | 1/1 |  |
| `gh run cancel` | a | PASS | 1/1 | Actions runs |
| `gh run delete` | a | PASS | 1/1 | Actions runs |
| `gh run download` | a | PASS | 3/3 | Actions runs |
| `gh run list` | a | PASS | 3/3 | Actions runs |
| `gh run rerun` | a | PASS | 2/2 | Actions runs |
| `gh run view` | a | PASS | 7/7 | Actions runs |
| `gh run watch` | a | PASS | 3/3 | Actions runs |
| `gh search code` | a | PASS | 2/2 |  |
| `gh search commits` | a | PASS | 2/2 |  |
| `gh search issues` | a | PASS | 2/2 |  |
| `gh search prs` | a | PASS | 2/2 |  |
| `gh search repos` | a | PASS | 2/2 |  |
| `gh secret delete` | a | PASS | 3/3 | Actions secrets |
| `gh secret list` | a | PASS | 2/2 | Actions secrets |
| `gh secret set` | a | PASS | 2/2 | Actions secrets |
| `gh skill install` | b | n/a | 0/0 | gh refuses non-github.com hosts (source.ValidateSupportedHost: "GitHub Skills does not currently support GitHub Enterprise Server") |
| `gh skill list` | a | PASS | 1/1 | local-only (lists installed skills) |
| `gh skill preview` | b | n/a | 0/0 | gh refuses non-github.com hosts (source.ValidateSupportedHost: "GitHub Skills does not currently support GitHub Enterprise Server") |
| `gh skill publish` | b | n/a | 0/0 | gh refuses non-github.com hosts (source.ValidateSupportedHost: "GitHub Skills does not currently support GitHub Enterprise Server") |
| `gh skill search` | b | n/a | 0/0 | gh refuses non-github.com hosts (source.ValidateSupportedHost: "GitHub Skills does not currently support GitHub Enterprise Server") |
| `gh skill update` | b | n/a | 0/0 | gh refuses non-github.com hosts (source.ValidateSupportedHost: "GitHub Skills does not currently support GitHub Enterprise Server") |
| `gh ssh-key add` | a | PASS | 2/2 |  |
| `gh ssh-key delete` | a | PASS | 2/2 |  |
| `gh ssh-key list` | a | PASS | 1/1 |  |
| `gh status` | a | PASS | 2/2 |  |
| `gh variable delete` | a | PASS | 1/1 | Actions variables |
| `gh variable get` | a | PASS | 1/1 | Actions variables |
| `gh variable list` | a | PASS | 2/2 | Actions variables |
| `gh variable set` | a | PASS | 3/3 | Actions variables |
| `gh workflow disable` | a | PASS | 2/2 | Actions workflows |
| `gh workflow enable` | a | PASS | 2/2 | Actions workflows |
| `gh workflow list` | a | PASS | 2/2 | Actions workflows |
| `gh workflow run` | a | PASS | 2/2 | Actions workflows |
| `gh workflow view` | a | PASS | 1/1 | Actions workflows |

## Cases

| # | Case | Command | Result | Exit | Notes |
|---|------|---------|--------|------|-------|
| 1 | auth status | auth status | PASS | 0 |  |
| 2 | api user (REST GET) | api | PASS | 0 |  |
| 3 | api graphql query | api | PASS | 0 |  |
| 4 | repo create (GraphQL) | repo create | PASS | 0 |  |
| 5 | repo create --add-readme (REST) | repo create | PASS | 0 |  |
| 6 | repo view | repo view | PASS | 0 |  |
| 7 | repo view (README) | repo view | PASS | 0 |  |
| 8 | repo view --json | repo view | PASS | 0 |  |
| 9 | repo list | repo list | PASS | 0 |  |
| 10 | repo list OWNER --json | repo list | PASS | 0 |  |
| 11 | repo edit | repo edit | PASS | 0 |  |
| 12 | repo view after edit | repo view | PASS | 0 |  |
| 13 | repo clone | repo clone | PASS | 0 |  |
| 14 | repo clone result | repo clone | PASS | 0 |  |
| 15 | repo fork (as other user) | repo fork | PASS | 0 |  |
| 16 | repo list --fork (other user) | repo list | PASS | 0 |  |
| 17 | label create (REST) | label create | PASS | 0 |  |
| 18 | label create --force (REST) | label create | PASS | 0 |  |
| 19 | api graphql mutation (createLabel) | api | PASS | 0 |  |
| 20 | label list | label list | PASS | 0 |  |
| 21 | label list --json | label list | PASS | 0 |  |
| 22 | issue create | issue create | PASS | 0 |  |
| 23 | issue create with metadata | issue create | PASS | 0 |  |
| 24 | issue create (to delete) | issue create | PASS | 0 |  |
| 25 | issue list | issue list | PASS | 0 |  |
| 26 | issue list --json | issue list | PASS | 0 |  |
| 27 | issue list --label (search) | issue list | PASS | 0 |  |
| 28 | issue list --search | issue list | PASS | 0 |  |
| 29 | issue list --assignee | issue list | PASS | 0 |  |
| 30 | issue view | issue view | PASS | 0 |  |
| 31 | issue comment | issue comment | PASS | 0 |  |
| 32 | issue view --comments | issue view | PASS | 0 |  |
| 33 | issue view --json | issue view | PASS | 0 |  |
| 34 | issue edit | issue edit | PASS | 0 |  |
| 35 | issue edit --remove-label | issue edit | PASS | 0 |  |
| 36 | issue view after edit | issue view | PASS | 0 |  |
| 37 | issue close | issue close | PASS | 0 |  |
| 38 | issue list --state closed | issue list | PASS | 0 |  |
| 39 | issue reopen | issue reopen | PASS | 0 |  |
| 40 | issue status | issue status | PASS | 0 |  |
| 41 | issue delete | issue delete | PASS | 0 |  |
| 42 | issue view deleted (404) | issue view | PASS | 1 |  |
| 43 | pr create | pr create | PASS | 0 |  |
| 44 | pr create (reviewer, label, assignee) | pr create | PASS | 0 |  |
| 45 | pr create --draft | pr create | PASS | 0 |  |
| 46 | pr create (4th) | pr create | PASS | 0 |  |
| 47 | pr create duplicate fails | pr create | PASS | 1 |  |
| 48 | pr list | pr list | PASS | 0 |  |
| 49 | pr list --json | pr list | PASS | 0 |  |
| 50 | pr list --author (search) | pr list | PASS | 0 |  |
| 51 | pr list --draft (search) | pr list | PASS | 0 |  |
| 52 | pr view | pr view | PASS | 0 |  |
| 53 | pr view --json | pr view | PASS | 0 |  |
| 54 | pr view reviewers | pr view | PASS | 0 |  |
| 55 | pr diff | pr diff | PASS | 0 |  |
| 56 | pr diff --name-only | pr diff | PASS | 0 |  |
| 57 | pr comment | pr comment | PASS | 0 |  |
| 58 | pr review --comment | pr review | PASS | 0 |  |
| 59 | pr review --approve (other user) | pr review | PASS | 0 |  |
| 60 | pr review --request-changes (other user) | pr review | PASS | 0 |  |
| 61 | pr view review decision | pr view | PASS | 0 |  |
| 62 | pr edit | pr edit | PASS | 0 |  |
| 63 | pr ready --undo | pr ready | PASS | 0 |  |
| 64 | pr ready | pr ready | PASS | 0 |  |
| 65 | pr view after ready | pr view | PASS | 0 |  |
| 66 | pr ready (undo the undo) | pr ready | PASS | 0 |  |
| 67 | pr checks | pr checks | PASS | 0 |  |
| 68 | pr checkout | pr checkout | PASS | 0 |  |
| 69 | pr checkout result | pr checkout | PASS | 0 |  |
| 70 | pr status | pr status | PASS | 0 |  |
| 71 | pr view (current branch) | pr view | PASS | 0 |  |
| 72 | pr merge --merge --delete-branch | pr merge | PASS | 0 |  |
| 73 | pr merge result (local main) | pr merge | PASS | 0 |  |
| 74 | pr view merged | pr view | PASS | 0 |  |
| 75 | pr close | pr close | PASS | 0 |  |
| 76 | pr reopen | pr reopen | PASS | 0 |  |
| 77 | pr merge --squash | pr merge | PASS | 0 |  |
| 78 | pr merge --rebase | pr merge | PASS | 0 |  |
| 79 | pr list --state merged | pr list | PASS | 0 |  |
| 80 | pr merge result (remote log) | api | PASS | 0 |  |
| 81 | release create | release create | PASS | 0 |  |
| 82 | release list | release list | PASS | 0 |  |
| 83 | release view | release view | PASS | 0 |  |
| 84 | api POST -f (REST) | api | PASS | 0 |  |
| 85 | api --paginate (REST) | api | PASS | 0 |  |
| 86 | api graphql --paginate | api | PASS | 0 |  |
| 87 | repo delete | repo delete | PASS | 0 |  |
| 88 | repo view deleted (404) | repo view | PASS | 1 |  |
| 89 | auth login --with-token | auth login | PASS | 0 |  |
| 90 | auth status (stored token) | auth status | PASS | 0 |  |
| 91 | auth login (second account) | auth login | PASS | 0 |  |
| 92 | auth status (two accounts) | auth status | PASS | 0 |  |
| 93 | auth switch | auth switch | PASS | 0 |  |
| 94 | auth token | auth token | PASS | 0 |  |
| 95 | auth token --user | auth token | PASS | 0 |  |
| 96 | auth setup-git | auth setup-git | PASS | 0 |  |
| 97 | auth setup-git result | auth setup-git | PASS | 0 |  |
| 98 | auth refresh (device flow) | auth refresh | PASS | 0 |  |
| 99 | auth refresh result | auth refresh | PASS | 0 |  |
| 100 | auth logout | auth logout | PASS | 0 |  |
| 101 | auth logout result | auth logout | PASS | 0 |  |
| 102 | browse -n (repo) | browse | PASS | 0 |  |
| 103 | browse -n (issue) | browse | PASS | 0 |  |
| 104 | browse -n (file) | browse | PASS | 0 |  |
| 105 | browse -n --settings | browse | PASS | 0 |  |
| 106 | browse -n --releases | browse | PASS | 0 |  |
| 107 | browse -n --commit | browse | PASS | 0 |  |
| 108 | repo archive | repo archive | PASS | 0 |  |
| 109 | repo archive result | repo archive | PASS | 0 |  |
| 110 | repo unarchive | repo unarchive | PASS | 0 |  |
| 111 | repo unarchive result | repo unarchive | PASS | 0 |  |
| 112 | repo rename | repo rename | PASS | 0 |  |
| 113 | repo rename result | repo rename | PASS | 0 |  |
| 114 | repo edit --add-topic | repo edit | PASS | 0 |  |
| 115 | repo view --json (topics, more fields) | repo view | PASS | 0 |  |
| 116 | repo list --json --jq | repo list | PASS | 0 |  |
| 117 | repo list --topic | repo list | PASS | 0 |  |
| 118 | repo set-default | repo set-default | PASS | 0 |  |
| 119 | repo set-default --view | repo set-default | PASS | 0 |  |
| 120 | repo sync (local) | repo sync | PASS | 0 |  |
| 121 | repo sync (local) result | repo sync | PASS | 0 |  |
| 122 | repo sync (fork from parent) | repo sync | PASS | 0 |  |
| 123 | repo sync (fork) result | repo sync | PASS | 0 |  |
| 124 | repo deploy-key add | repo deploy-key add | PASS | 0 |  |
| 125 | repo deploy-key list | repo deploy-key list | PASS | 0 |  |
| 126 | repo deploy-key list --json | repo deploy-key list | PASS | 0 |  |
| 127 | repo deploy-key delete | repo deploy-key delete | PASS | 0 |  |
| 128 | repo autolink create | repo autolink create | PASS | 0 |  |
| 129 | repo autolink list | repo autolink list | PASS | 0 |  |
| 130 | repo autolink list --json | repo autolink list | PASS | 0 |  |
| 131 | repo autolink view | repo autolink view | PASS | 0 |  |
| 132 | repo autolink delete | repo autolink delete | PASS | 0 |  |
| 133 | repo gitignore list | repo gitignore list | PASS | 0 |  |
| 134 | repo gitignore view | repo gitignore view | PASS | 0 |  |
| 135 | repo license list | repo license list | PASS | 0 |  |
| 136 | repo license view | repo license view | PASS | 0 |  |
| 137 | repo read-file | repo read-file | PASS | 0 |  |
| 138 | repo read-file --ref --json | repo read-file | PASS | 0 |  |
| 139 | repo read-dir | repo read-dir | PASS | 0 |  |
| 140 | repo read-dir PATH --json | repo read-dir | PASS | 0 |  |
| 141 | issue create (for transfer) | issue create | PASS | 0 |  |
| 142 | issue transfer | issue transfer | PASS | 0 |  |
| 143 | issue transfer result | issue transfer | PASS | 0 |  |
| 144 | issue transfer (gone from source) | issue transfer | PASS | 1 |  |
| 145 | issue pin | issue pin | PASS | 0 |  |
| 146 | issue pin result | issue pin | PASS | 0 |  |
| 147 | issue unpin | issue unpin | PASS | 0 |  |
| 148 | issue unpin result | issue unpin | PASS | 0 |  |
| 149 | issue lock | issue lock | PASS | 0 |  |
| 150 | issue lock result | issue lock | PASS | 0 |  |
| 151 | issue unlock | issue unlock | PASS | 0 |  |
| 152 | issue unlock result | issue unlock | PASS | 0 |  |
| 153 | issue develop | issue develop | PASS | 0 |  |
| 154 | issue develop --list | issue develop | PASS | 0 |  |
| 155 | issue develop result | issue develop | PASS | 0 |  |
| 156 | issue comment --edit-last | issue comment | PASS | 0 |  |
| 157 | issue status --json | issue status | PASS | 0 |  |
| 158 | issue list --json --jq | issue list | PASS | 0 |  |
| 159 | issue create --milestone | issue create | PASS | 0 |  |
| 160 | issue list --milestone | issue list | PASS | 0 |  |
| 161 | issue edit (two issues) | issue edit | PASS | 0 |  |
| 162 | issue close --json view | issue close | PASS | 0 |  |
| 163 | issue reopen (again) | issue reopen | PASS | 0 |  |
| 164 | pr lock | pr lock | PASS | 0 |  |
| 165 | pr lock result | pr lock | PASS | 0 |  |
| 166 | pr unlock | pr unlock | PASS | 0 |  |
| 167 | pr unlock result | pr unlock | PASS | 0 |  |
| 168 | pr update-branch | pr update-branch | PASS | 0 |  |
| 169 | pr update-branch result | pr update-branch | PASS | 0 |  |
| 170 | pr create (behind base) | pr create | PASS | 0 |  |
| 171 | pr update-branch --rebase | pr update-branch | PASS | 0 |  |
| 172 | pr update-branch --rebase result | pr update-branch | PASS | 0 |  |
| 173 | pr revert | pr revert | PASS | 0 |  |
| 174 | pr revert result | pr revert | PASS | 0 |  |
| 175 | pr revert diff | pr revert | PASS | 0 |  |
| 176 | pr comment --edit-last | pr comment | PASS | 0 |  |
| 177 | pr view --comments | pr view | PASS | 0 |  |
| 178 | pr status --json | pr status | PASS | 0 |  |
| 179 | pr checks --json | pr checks | PASS | 0 |  |
| 180 | pr diff --patch | pr diff | PASS | 0 |  |
| 181 | pr list --json --jq | pr list | PASS | 0 |  |
| 182 | pr list --label | pr list | PASS | 0 |  |
| 183 | pr edit --add-reviewer | pr edit | PASS | 0 |  |
| 184 | pr review --request-changes -> approve | pr review | PASS | 0 |  |
| 185 | label edit | label edit | PASS | 0 |  |
| 186 | label edit result | label edit | PASS | 0 |  |
| 187 | label clone | label clone | PASS | 0 |  |
| 188 | label clone result | label clone | PASS | 0 |  |
| 189 | label delete | label delete | PASS | 0 |  |
| 190 | label delete result | label delete | PASS | 1 |  |
| 191 | release create (with asset) | release create | PASS | 0 |  |
| 192 | release create --draft --generate-notes | release create | PASS | 0 |  |
| 193 | release upload | release upload | PASS | 0 |  |
| 194 | release view --json | release view | PASS | 0 |  |
| 195 | release list --json | release list | PASS | 0 |  |
| 196 | release download (pattern) | release download | PASS | 0 |  |
| 197 | release download result | release download | PASS | 0 |  |
| 198 | release download --archive | release download | PASS | 0 |  |
| 199 | release download --archive result | release download | PASS | 0 |  |
| 200 | release edit | release edit | PASS | 0 |  |
| 201 | release edit result | release edit | PASS | 0 |  |
| 202 | release delete-asset | release delete-asset | PASS | 0 |  |
| 203 | release delete-asset result | release delete-asset | PASS | 0 |  |
| 204 | release delete | release delete | PASS | 0 |  |
| 205 | release delete result | release delete | PASS | 1 |  |
| 206 | gist create | gist create | PASS | 0 |  |
| 207 | gist create (stdin) | gist create | PASS | 0 |  |
| 208 | gist list | gist list | PASS | 0 |  |
| 209 | gist list --public | gist list | PASS | 0 |  |
| 210 | gist view | gist view | PASS | 0 |  |
| 211 | gist view --files | gist view | PASS | 0 |  |
| 212 | gist edit --add | gist edit | PASS | 0 |  |
| 213 | gist edit FILE (replace content) | gist edit | PASS | 0 |  |
| 214 | gist edit result | gist edit | PASS | 0 |  |
| 215 | gist edit --desc | gist edit | PASS | 0 |  |
| 216 | gist rename | gist rename | PASS | 0 |  |
| 217 | gist rename result | gist rename | PASS | 0 |  |
| 218 | gist clone | gist clone | PASS | 0 |  |
| 219 | gist clone result | gist clone | PASS | 0 |  |
| 220 | gist delete | gist delete | PASS | 0 |  |
| 221 | gist delete result | gist delete | PASS | 1 |  |
| 222 | ssh-key add | ssh-key add | PASS | 0 |  |
| 223 | ssh-key add --type signing | ssh-key add | PASS | 0 |  |
| 224 | ssh-key list | ssh-key list | PASS | 0 |  |
| 225 | ssh-key delete | ssh-key delete | PASS | 0 |  |
| 226 | ssh-key delete result | ssh-key delete | PASS | 0 |  |
| 227 | gpg-key add | gpg-key add | PASS | 0 |  |
| 228 | gpg-key list | gpg-key list | PASS | 0 |  |
| 229 | gpg-key delete | gpg-key delete | PASS | 0 |  |
| 230 | gpg-key delete result | gpg-key delete | PASS | 0 |  |
| 231 | org list | org list | PASS | 0 |  |
| 232 | search repos | search repos | PASS | 0 |  |
| 233 | search repos --json | search repos | PASS | 0 |  |
| 234 | search issues | search issues | PASS | 0 |  |
| 235 | search issues --json | search issues | PASS | 0 |  |
| 236 | search prs | search prs | PASS | 0 |  |
| 237 | search prs --json --state | search prs | PASS | 0 |  |
| 238 | search commits | search commits | PASS | 0 |  |
| 239 | search commits --json | search commits | PASS | 0 |  |
| 240 | search code | search code | PASS | 0 |  |
| 241 | search code --json | search code | PASS | 0 |  |
| 242 | status | status | PASS | 0 |  |
| 243 | status --org | status | PASS | 0 |  |
| 244 | api -X PATCH | api | PASS | 0 |  |
| 245 | api --include | api | PASS | 0 |  |
| 246 | api --template | api | PASS | 0 |  |
| 247 | api --input | api | PASS | 0 |  |
| 248 | api graphql -F variables | api | PASS | 0 |  |
| 249 | api 404 | api | PASS | 1 |  |
| 250 | ruleset list | ruleset list | PASS | 0 |  |
| 251 | ruleset view | ruleset view | PASS | 0 |  |
| 252 | ruleset check | ruleset check | PASS | 0 |  |
| 253 | workflow list | workflow list | PASS | 0 |  |
| 254 | workflow list --json | workflow list | PASS | 0 |  |
| 255 | workflow view | workflow view | PASS | 0 |  |
| 256 | workflow disable | workflow disable | PASS | 0 |  |
| 257 | workflow disable result | workflow disable | PASS | 0 |  |
| 258 | workflow enable | workflow enable | PASS | 0 |  |
| 259 | workflow enable result | workflow enable | PASS | 0 |  |
| 260 | workflow run | workflow run | PASS | 0 |  |
| 261 | run list | run list | PASS | 0 |  |
| 262 | run list --json | run list | PASS | 0 |  |
| 263 | run view | run view | PASS | 0 |  |
| 264 | run view --json | run view | PASS | 0 |  |
| 265 | run cancel | run cancel | PASS | 0 |  |
| 266 | run watch | run watch | PASS | 0 |  |
| 267 | run rerun | run rerun | PASS | 0 |  |
| 268 | run download | run download | PASS | 1 |  |
| 269 | run delete | run delete | PASS | 0 |  |
| 270 | cache list | cache list | PASS | 0 |  |
| 271 | cache list --json | cache list | PASS | 0 |  |
| 272 | cache delete --all | cache delete | PASS | 0 |  |
| 273 | secret set | secret set | PASS | 0 |  |
| 274 | secret list | secret list | PASS | 0 |  |
| 275 | secret list --json | secret list | PASS | 0 |  |
| 276 | secret delete | secret delete | PASS | 0 |  |
| 277 | secret delete result | secret delete | PASS | 0 |  |
| 278 | variable set | variable set | PASS | 0 |  |
| 279 | variable list | variable list | PASS | 0 |  |
| 280 | variable list --json | variable list | PASS | 0 |  |
| 281 | variable get | variable get | PASS | 0 |  |
| 282 | variable set (update) | variable set | PASS | 0 |  |
| 283 | variable set result | variable set | PASS | 0 |  |
| 284 | variable delete | variable delete | PASS | 0 |  |
| 285 | secret set (for a job) | secret set | PASS | 0 |  |
| 286 | workflow run (runner) | workflow run | PASS | 0 |  |
| 287 | run watch --exit-status (failure) | run watch | PASS | 1 |  |
| 288 | run view (completed jobs) | run view | PASS | 0 |  |
| 289 | run view --log | run view | PASS | 0 |  |
| 290 | run view --log (secret masked) | run view | PASS | 0 |  |
| 291 | run view --log-failed | run view | PASS | 0 |  |
| 292 | run download (artifact) | run download | PASS | 0 |  |
| 293 | run download result | run download | PASS | 0 |  |
| 294 | run rerun --failed | run rerun | PASS | 0 |  |
| 295 | run watch --exit-status (rerun succeeds) | run watch | PASS | 0 |  |
| 296 | run list --json (completed) | run list | PASS | 0 |  |
| 297 | run view --json jobs (attempt 2) | run view | PASS | 0 |  |
| 298 | secret delete (job secret) | secret delete | PASS | 0 |  |
| 299 | alias set | alias set | PASS | 0 |  |
| 300 | alias set (use) | alias set | PASS | 0 |  |
| 301 | alias list | alias list | PASS | 0 |  |
| 302 | alias import | alias import | PASS | 0 |  |
| 303 | alias import (use) | alias import | PASS | 0 |  |
| 304 | alias delete | alias delete | PASS | 0 |  |
| 305 | config set | config set | PASS | 0 |  |
| 306 | config get | config get | PASS | 0 |  |
| 307 | config list | config list | PASS | 0 |  |
| 308 | config clear-cache | config clear-cache | PASS | 0 |  |
| 309 | completion | completion | PASS | 0 |  |
| 310 | licenses | licenses | PASS | 0 |  |
| 311 | extension install | extension install | PASS | 0 |  |
| 312 | extension list | extension list | PASS | 0 |  |
| 313 | extension exec | extension exec | PASS | 0 |  |
| 314 | extension upgrade | extension upgrade | PASS | 0 |  |
| 315 | extension upgrade result | extension upgrade | PASS | 0 |  |
| 316 | extension search | extension search | PASS | 0 |  |
| 317 | extension remove | extension remove | PASS | 0 |  |
| 318 | extension create | extension create | PASS | 0 |  |
| 319 | extension create result | extension create | PASS | 0 |  |
| 320 | skill list | skill list | PASS | 0 |  |
| 321 | repo edit --enable-discussions | repo edit | PASS | 0 |  |
| 322 | discussion create | discussion create | PASS | 0 |  |
| 323 | discussion create (Q&A) | discussion create | PASS | 0 |  |
| 324 | discussion list | discussion list | PASS | 0 |  |
| 325 | discussion list --json | discussion list | PASS | 0 |  |
| 326 | discussion list --category | discussion list | PASS | 0 |  |
| 327 | discussion view | discussion view | PASS | 0 |  |
| 328 | discussion comment | discussion comment | PASS | 0 |  |
| 329 | discussion view --comments | discussion view | PASS | 0 |  |
| 330 | discussion edit | discussion edit | PASS | 0 |  |
| 331 | discussion edit result | discussion edit | PASS | 0 |  |
| 332 | project create | project create | PASS | 0 |  |
| 333 | project list | project list | PASS | 0 |  |
| 334 | project list --format json | project list | PASS | 0 |  |
| 335 | project view | project view | PASS | 0 |  |
| 336 | project edit | project edit | PASS | 0 |  |
| 337 | project field-create | project field-create | PASS | 0 |  |
| 338 | project field-create (text) | project field-create | PASS | 0 |  |
| 339 | project field-list | project field-list | PASS | 0 |  |
| 340 | project item-create | project item-create | PASS | 0 |  |
| 341 | project item-add | project item-add | PASS | 0 |  |
| 342 | project item-list | project item-list | PASS | 0 |  |
| 343 | project item-edit (draft) | project item-edit | PASS | 0 |  |
| 344 | project item-edit (field by name) | project item-edit | PASS | 0 |  |
| 345 | project item-edit result | project item-edit | PASS | 0 |  |
| 346 | project item-list --query | project item-list | PASS | 0 |  |
| 347 | project item-archive | project item-archive | PASS | 0 |  |
| 348 | project item-archive result | project item-archive | PASS | 0 |  |
| 349 | project item-delete | project item-delete | PASS | 0 |  |
| 350 | project item-delete result | project item-delete | PASS | 0 |  |
| 351 | project field-delete | project field-delete | PASS | 0 |  |
| 352 | project field-delete result | project field-delete | PASS | 1 |  |
| 353 | project link | project link | PASS | 0 |  |
| 354 | project link result | project link | PASS | 0 |  |
| 355 | project unlink | project unlink | PASS | 0 |  |
| 356 | project unlink result | project unlink | PASS | 0 |  |
| 357 | project copy | project copy | PASS | 0 |  |
| 358 | project mark-template | project mark-template | PASS | 0 |  |
| 359 | project mark-template result | project mark-template | PASS | 0 |  |
| 360 | project close | project close | PASS | 0 |  |
| 361 | project close --undo | project close | PASS | 0 |  |
| 362 | project delete | project delete | PASS | 0 |  |
| 363 | project delete result | project delete | PASS | 1 |  |

# gh command coverage

The oracle is "% of gh commands that work against forge". This file defines the denominator.
The machine-readable catalogue is `tests/gh/commands.tsv`; `tests/gh/run.sh` attributes every
case to one command and writes per-command results to `tests/gh/report.md`.

## Denominator

- **Universe.** Every runnable command of gh **v2.102.0**, taken from `gh help reference`.
  That is each leaf command, plus `codespace ports`, which runs on its own and also has
  subcommands. That gives **197 commands**. Not counted: the built-in `co` alias (it is
  `pr checkout`), hidden commands (`version`, `send-telemetry`) and help topics.
- **Category (a), forge-relevant: 166 commands.** These are counted in the denominator. The
  category covers everything that operates on data a GitHub-compatible forge hosts: repos,
  issues, PRs, labels, releases, gists, keys, orgs, search, rulesets, Actions metadata,
  discussions and projects. It also covers the local-only commands (alias, config, completion,
  extension, licenses, `skill list`), which must simply work.
- **Category (b), not applicable: 31 commands.** These are excluded, each for one of three
  reasons:
  - It depends on a github.com-only service (Codespaces, Copilot, the agent-task Copilot coding
    agent, Sigstore attestations).
  - gh itself refuses non-github.com hosts.
  - It is a TTY-only UI with nothing for a server to implement.
- **A command "works"** when it has at least one case in `run.sh` and all of its cases pass.
  Cases assert on results, not just exit codes: output, `--json` fields, or a follow-up
  read-back through the API. Several commands have more than one case.

`run.sh` prints three numbers:
- `PASS n/166`: forge-relevant commands that work. This is the headline number.
- `n/197 of all gh commands`
- the per-case pass rate

## Category (b): not applicable, and why

| Command(s) | Reason |
|---|---|
| `agent-task create/list/view` (3) | The Copilot coding agent, a github.com service (api.githubcopilot.com). |
| `attestation download/trusted-root/verify` (3) | Sigstore and GitHub artifact attestations. They need the public-good Sigstore and GitHub's attestation store. |
| `release verify`, `release verify-asset` (2) | These verify Sigstore release attestations, which is the same dependency. |
| `codespace *` (16, including `codespace ports`) | GitHub Codespaces, a hosted VM service. |
| `copilot` (1) | Downloads and runs the GitHub Copilot CLI. |
| `skill install/preview/publish/search/update` (5) | gh rejects every non-github.com host before sending a request: `source.ValidateSupportedHost` prints "GitHub Skills does not currently support GitHub Enterprise Server". `skill list` only reads a local directory, so it stays in (a). |
| `extension browse` (1) | A full-screen TUI that needs a terminal. The data it shows comes from `extension search`, which is in (a). |
| `preview prompter` (1) | A developer preview of gh's own interactive prompts. It makes no host requests and needs a terminal. |

## Notes on category (a) choices

- **Projects (v2) and Discussions.** Both ship with GitHub Enterprise Server and gh drives them
  through standard GraphQL, so they count. They are not marked N/A just because forge lacked
  them.
- **Projects (v2)** are GraphQL-only, like GitHub (`graphql::projects`, `api::projects_v2`).
  A user's projects are managed by that user; an organization's by its admins, and members
  can read and create them. Public projects are readable by everyone. New projects get
  GitHub's default fields (Title, Assignees, Status, Labels, Linked pull requests, Milestone,
  Repository, Reviewers). Custom fields can be text, number, date, single-select or iteration.
  Title, Assignees, Labels, Milestone and Repository values come from the issue or pull
  request. Linked-pull-request and reviewer values are not populated yet. `items(query:)`
  supports free text, `is:` qualifiers and `<field>:<value>`. Team links return an error,
  because forge has no team–project links.
- **Actions.** forge has no runner. Workflows come from `.github/workflows` and can be enabled,
  disabled and dispatched. A dispatch records a queued run, which can be listed, viewed,
  cancelled, re-run, watched and deleted. Jobs, logs and artifacts are validly empty, so
  `gh run download` correctly reports "no valid artifacts". Caches are always empty.
- **Secrets and variables.** Repository, environment and org scopes are stored. Secrets keep
  only the client-side ciphertext, which forge never decrypts. `secret set --user` writes
  Codespaces user secrets (`/user/codespaces/secrets`), so the suite tests the Actions scopes
  instead.
- **Auth.** `gh auth login` is tested with `--with-token`. `gh auth refresh` (and
  `login --web`) use the OAuth device flow (`/login/device/code`,
  `/login/oauth/access_token`). The suite approves the one-time code through
  `POST /api/forge/device/approve`, which is what the `/login/device` web page calls.
- **`browse`.** It is tested with `--no-browser`, which prints the URL.

## Full catalogue

`a` = forge-relevant (counted), `b` = not applicable. Live per-command results are in
`tests/gh/report.md`.

| Command | Cat. | Note |
|---|---|---|
| `gh agent-task create` | b | Copilot coding agent |
| `gh agent-task list` | b | Copilot coding agent |
| `gh agent-task view` | b | Copilot coding agent |
| `gh alias delete` | a | local |
| `gh alias import` | a | local |
| `gh alias list` | a | local |
| `gh alias set` | a | local |
| `gh api` | a | REST + GraphQL, pagination, `--input`, `--template`, `-i` |
| `gh attestation download` | b | Sigstore |
| `gh attestation trusted-root` | b | Sigstore TUF root |
| `gh attestation verify` | b | Sigstore |
| `gh auth login` | a | `--with-token` |
| `gh auth logout` | a | |
| `gh auth refresh` | a | OAuth device flow |
| `gh auth setup-git` | a | |
| `gh auth status` | a | |
| `gh auth switch` | a | |
| `gh auth token` | a | |
| `gh browse` | a | `--no-browser` |
| `gh cache delete` | a | Actions (empty) |
| `gh cache list` | a | Actions (empty) |
| `gh codespace code` | b | Codespaces |
| `gh codespace cp` | b | Codespaces |
| `gh codespace create` | b | Codespaces |
| `gh codespace delete` | b | Codespaces |
| `gh codespace edit` | b | Codespaces |
| `gh codespace jupyter` | b | Codespaces |
| `gh codespace list` | b | Codespaces |
| `gh codespace logs` | b | Codespaces |
| `gh codespace ports` | b | Codespaces |
| `gh codespace ports forward` | b | Codespaces |
| `gh codespace ports visibility` | b | Codespaces |
| `gh codespace rebuild` | b | Codespaces |
| `gh codespace ssh` | b | Codespaces |
| `gh codespace stop` | b | Codespaces |
| `gh codespace view` | b | Codespaces |
| `gh completion` | a | local |
| `gh config clear-cache` | a | local |
| `gh config get` | a | local |
| `gh config list` | a | local |
| `gh config set` | a | local |
| `gh copilot` | b | Copilot CLI |
| `gh discussion comment` | a | |
| `gh discussion create` | a | |
| `gh discussion edit` | a | |
| `gh discussion list` | a | |
| `gh discussion view` | a | |
| `gh extension browse` | b | TTY-only TUI |
| `gh extension create` | a | local |
| `gh extension exec` | a | local |
| `gh extension install` | a | from a forge repository |
| `gh extension list` | a | local |
| `gh extension remove` | a | local |
| `gh extension search` | a | repo search `topic:gh-extension` |
| `gh extension upgrade` | a | fetches from forge |
| `gh gist clone` | a | read-only git over HTTP |
| `gh gist create` | a | |
| `gh gist delete` | a | |
| `gh gist edit` | a | |
| `gh gist list` | a | |
| `gh gist rename` | a | |
| `gh gist view` | a | |
| `gh gpg-key add` | a | |
| `gh gpg-key delete` | a | |
| `gh gpg-key list` | a | |
| `gh issue close` | a | |
| `gh issue comment` | a | |
| `gh issue create` | a | |
| `gh issue delete` | a | |
| `gh issue develop` | a | linked branches |
| `gh issue edit` | a | |
| `gh issue list` | a | |
| `gh issue lock` | a | |
| `gh issue pin` | a | |
| `gh issue reopen` | a | |
| `gh issue status` | a | |
| `gh issue transfer` | a | |
| `gh issue unlock` | a | |
| `gh issue unpin` | a | |
| `gh issue view` | a | |
| `gh label clone` | a | |
| `gh label create` | a | |
| `gh label delete` | a | |
| `gh label edit` | a | |
| `gh label list` | a | |
| `gh licenses` | a | local |
| `gh org list` | a | |
| `gh pr checkout` | a | |
| `gh pr checks` | a | |
| `gh pr close` | a | |
| `gh pr comment` | a | |
| `gh pr create` | a | |
| `gh pr diff` | a | |
| `gh pr edit` | a | |
| `gh pr list` | a | |
| `gh pr lock` | a | |
| `gh pr merge` | a | |
| `gh pr ready` | a | |
| `gh pr reopen` | a | |
| `gh pr revert` | a | |
| `gh pr review` | a | |
| `gh pr status` | a | |
| `gh pr unlock` | a | |
| `gh pr update-branch` | a | merge and `--rebase` |
| `gh pr view` | a | |
| `gh preview prompter` | b | TTY-only developer preview |
| `gh project close` | a | Projects v2 |
| `gh project copy` | a | Projects v2 |
| `gh project create` | a | Projects v2 |
| `gh project delete` | a | Projects v2 |
| `gh project edit` | a | Projects v2 |
| `gh project field-create` | a | Projects v2 |
| `gh project field-delete` | a | Projects v2 |
| `gh project field-list` | a | Projects v2 |
| `gh project item-add` | a | Projects v2 |
| `gh project item-archive` | a | Projects v2 |
| `gh project item-create` | a | Projects v2 |
| `gh project item-delete` | a | Projects v2 |
| `gh project item-edit` | a | Projects v2 |
| `gh project item-list` | a | Projects v2 |
| `gh project link` | a | Projects v2 |
| `gh project list` | a | Projects v2 |
| `gh project mark-template` | a | Projects v2 |
| `gh project unlink` | a | Projects v2 |
| `gh project view` | a | Projects v2 |
| `gh release create` | a | |
| `gh release delete` | a | |
| `gh release delete-asset` | a | |
| `gh release download` | a | |
| `gh release edit` | a | |
| `gh release list` | a | |
| `gh release upload` | a | |
| `gh release verify` | b | Sigstore attestations |
| `gh release verify-asset` | b | Sigstore attestations |
| `gh release view` | a | |
| `gh repo archive` | a | |
| `gh repo autolink create` | a | |
| `gh repo autolink delete` | a | |
| `gh repo autolink list` | a | |
| `gh repo autolink view` | a | |
| `gh repo clone` | a | |
| `gh repo create` | a | |
| `gh repo delete` | a | |
| `gh repo deploy-key add` | a | |
| `gh repo deploy-key delete` | a | |
| `gh repo deploy-key list` | a | |
| `gh repo edit` | a | |
| `gh repo fork` | a | |
| `gh repo gitignore list` | a | |
| `gh repo gitignore view` | a | |
| `gh repo license list` | a | |
| `gh repo license view` | a | |
| `gh repo list` | a | |
| `gh repo read-dir` | a | |
| `gh repo read-file` | a | |
| `gh repo rename` | a | |
| `gh repo set-default` | a | local (git config) |
| `gh repo sync` | a | local and fork-from-parent |
| `gh repo unarchive` | a | |
| `gh repo view` | a | |
| `gh ruleset check` | a | |
| `gh ruleset list` | a | |
| `gh ruleset view` | a | |
| `gh run cancel` | a | Actions (no runner) |
| `gh run delete` | a | Actions (no runner) |
| `gh run download` | a | Actions (no artifacts) |
| `gh run list` | a | Actions (no runner) |
| `gh run rerun` | a | Actions (no runner) |
| `gh run view` | a | Actions (no runner) |
| `gh run watch` | a | Actions (no runner) |
| `gh search code` | a | |
| `gh search commits` | a | |
| `gh search issues` | a | |
| `gh search prs` | a | |
| `gh search repos` | a | |
| `gh secret delete` | a | |
| `gh secret list` | a | |
| `gh secret set` | a | |
| `gh skill install` | b | gh refuses non-github.com hosts |
| `gh skill list` | a | local |
| `gh skill preview` | b | gh refuses non-github.com hosts |
| `gh skill publish` | b | gh refuses non-github.com hosts |
| `gh skill search` | b | gh refuses non-github.com hosts |
| `gh skill update` | b | gh refuses non-github.com hosts |
| `gh ssh-key add` | a | auth and signing keys |
| `gh ssh-key delete` | a | |
| `gh ssh-key list` | a | |
| `gh status` | a | |
| `gh variable delete` | a | |
| `gh variable get` | a | |
| `gh variable list` | a | |
| `gh variable set` | a | |
| `gh workflow disable` | a | |
| `gh workflow enable` | a | |
| `gh workflow list` | a | |
| `gh workflow run` | a | |
| `gh workflow view` | a | |

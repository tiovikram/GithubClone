# Web UI: actions area

Code: `web/src/areas/actions/` (registration in `index.tsx`). E2E: `web/e2e/areas/actions.mjs`
(30 steps incl. sign-up). It starts a real `forge runner` (shell executor) as a child process with
the registration token it reads from the "New self-hosted runner" page, pointed at `--base`. The
binary is `$FORGE_BIN`, else `<repo>/target/debug/forge`, else `data/web-actions/forge`. The runner
runs with `GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_GLOBAL=/dev/null` and is killed at the end.

```sh
scripts/web-area-server.sh actions 3430 5233
cd web && node e2e/run.mjs --base http://localhost:5233 --login <fresh> --password 'e2e-pass-123' --areas actions --only-areas
```

## Features

| Feature | UI location | E2E step (`actions: …`) |
|---|---|---|
| Actions tab, empty state | repo tab **Actions** (`/{o}/{r}/actions`) | empty Actions tab |
| Workflows sidebar, all runs, workflow runs | `/actions`, `/actions/workflows/{file}` | push a workflow, watch the run finish |
| Run filters: event, status, branch, actor (URL query) | runs list header | filter runs by event, status, branch and actor |
| Live run status (polls while queued/in progress/waiting) | run page | push a workflow… (status reaches Success without reload) |
| Run summary: trigger, status, duration, artifact count | `/actions/runs/{id}` | run summary shows… |
| Job graph from `needs` (matrix jobs grouped) | run summary | push a workflow… (`job-graph`) |
| Annotations (from the job's check run) | run summary, PR Checks page | run summary shows…, checks on a PR |
| Job summaries (`GITHUB_STEP_SUMMARY`, markdown) | run summary, PR Checks page | run summary shows…, checks on a PR |
| Artifacts: list, download (zip), delete | run summary → Artifacts | run summary shows…, delete an artifact |
| Log archive download (run attempt), raw job log | run "…" menu, job ⚙ menu | run summary shows…, job logs… |
| Jobs and steps with collapsible logs, groups, warnings/errors, search, live while running | `/actions/runs/{id}/job/{job_id}` | job logs with steps (secret masked, variable visible) |
| Re-run all jobs / failed jobs / one job; attempts selector | run header "Re-run jobs", job header, "Attempt #n" | re-run all jobs (attempt 2) |
| Cancel / force cancel | run header, "…" menu | cancel a running workflow and delete the run |
| Delete run (list menu and run menu), delete all logs | runs list "…", run "…" | cancel… and delete the run; delete a workflow run from the run page |
| workflow_dispatch form (branch, string/choice/boolean/number/environment inputs) | workflow page → "Run workflow" | dispatch the workflow with inputs |
| Enable / disable workflow | workflow page "…" menu | disable and enable a workflow |
| Caches: list, sort, filter by key, usage, delete | `/actions/caches` (sidebar "Management") | caches page lists and deletes caches |
| Runners (read-only view) | `/actions/runners` | (link to settings; covered by 375px check of settings) |
| Pending deployments: review (approve / reject with comment) | run summary "Review deployments" | approve a pending deployment in the UI |
| Environments: create, required reviewers, prevent self-review, wait timer, admin bypass, branch/tag policies, delete | Settings → Environments (`/settings/environments[/new|/{id}/edit]`) | environment with required reviewer, branch rule and secret |
| Environment secrets and variables | environment edit page | environment with… (`PROD_SECRET` used masked by the deploy job) |
| Deployments: sidebar on Code page, deployments page per environment with latest status | Code page sidebar, `/{o}/{r}/deployments[?environment=]` | deployments in the repo sidebar and deployments page |
| Repository secrets (sealed in the browser), update, remove | Settings → Actions secrets (`/settings/secrets/actions[/new|/NAME]`) | repository secret and variable created in the UI (job sees `***`) |
| Repository variables: create, edit, delete | Settings → Actions variables (`/settings/variables/actions`) | same step (job prints the value) |
| Organization secrets/variables available to the repo | repo secrets/variables pages (org repos) | organization Actions settings… |
| Checks on PRs: summary box with all check runs + commit statuses | PR conversation, above the merge box (slot `pull.merge.top`) | checks and statuses on a pull request |
| PR Checks page: check runs list, output, annotations, re-run | `/{o}/{r}/pull/{n}/checks` | checks and statuses on a pull request |
| Checks + statuses on a commit | commit page (slot `commit.header`) | checks and statuses on a pull request |
| Actions general settings: enable, allowed actions (+ selected actions patterns, SHA pinning), retention, fork PR approval, GITHUB_TOKEN permissions, access, OIDC subject claim | Settings → Actions → General (`/settings/actions`) | Actions general settings (permissions, retention, OIDC) |
| Runners: list, status, labels add/remove, remove runner | Settings → Actions → Runners | runner comes online, runner labels, remove the runner in the UI |
| New self-hosted runner: download, registration token, `forge runner` command | Settings → Actions → Runners → New self-hosted runner | new self-hosted runner page and runner comes online |
| Org Actions settings (General, Runners, Runner groups), runner groups create/edit/delete with repository access and runners | orgSettings `actions` (`/organizations/{org}/settings/actions[/runners|/runner-groups[/id|/new]]`) | organization Actions settings… (SKIP if the orgs shell is absent) |
| Org secrets and variables with repository access (all / private / selected repos) | orgSettings `secrets/actions`, `variables/actions` | organization Actions settings… |
| Enterprise Actions policies, runners, runner groups (organization access) | adminSections `actions` (`/site-admin/actions[/runners|/runner-groups]`) | site admin Actions policies and runner groups (SKIP unless site admin and the admin shell exists) |
| Permissions: read-only users see runs but no re-run/delete/settings | — | other users see runs but no admin controls |
| 375 px layout | 12 main pages | 375px pages have no horizontal overflow |

The org and site-admin steps were verified against a temporary stand-in shell (not committed)
with a site-admin login; in a run without the orgs/admin shells or without a site-admin user they
log a `SKIP` line. Settings sections match sub-pages against the absolute settings root (not
nested `<Routes>`), so they work however the hosting shell declares its route.

## Server changes

- `runner/agent.rs`, `runner/protocol.rs`: the runner sends the masked job summary
  (`GITHUB_STEP_SUMMARY`) on completion; the server stores it as the job's check run
  `output.text` (the run summary and the PR Checks page render it).
- `api/actions.rs` + `lib.rs`: forge-only `GET /api/forge/repos/{o}/{r}/actions/jobs/{id}/steps`
  → `{job_id, status, steps: [{number, log}]}` (per-step logs, live chunks while running).
- Test: `tests/runner.rs::job_summary_and_per_step_logs`.

Secrets are sealed client-side (libsodium `crypto_box_seal`): `sealed.ts` uses `tweetnacl@1.0.3`
(`nacl.box`) plus a small BLAKE2b for the nonce (checked against node's `blake2b512`).

## Left out

- **Fork PR approval** ("Approve and run" for first-time contributors): GHES 3.22 has no
  `POST /actions/runs/{id}/approve` and forge runs no `action_required` gate, so there is nothing
  to approve; the approval *policy* setting is available.
- **Commit status icons in commit lists**: the commit list is core (`components/CommitList.tsx`)
  and has no slot; adding one is a core change outside this area. Status is shown on the commit
  page and PRs.
- **Workflow templates (`/actions/new`) and in-browser workflow editing**: forge has no starter
  workflow catalogue; workflows are added by pushing files.
- **Usage / billing metrics, attestations**: not implemented by the server.
- Selecting individual repositories/organizations for "enabled repositories/organizations"
  (org/enterprise policy = selected) is API-only; the UI sets the mode.
- Enterprise slug: `AdminCtx.enterprise` is used when the admin shell provides it, else `forge`.

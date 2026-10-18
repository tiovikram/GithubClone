# REST conformance report

- **Target:** `http://localhost:3320/api/v3`
- **Spec:** `descriptions/ghes-3.22/ghes-3.22.json` @ [`58b1e0c00b`](https://github.com/github/rest-api-description/tree/58b1e0c00b39b9e46c24dc4a0ba6a2669c90e9c1) (API 1.1.4)
- **Run:** 2026-10-18T08:29:31.167Z · 31 HTTP requests · max tier 3

## Score

| scope | ops | conforming | nonconforming | not impl. | untested | % conforming | % of tested |
|---|---:|---:|---:|---:|---:|---:|---:|
| **all operations** | 1105 | 0 | 0 | 23 | 1082 | **0%** | 0% |
| tier 1 (gh CLI core) | 104 | 0 | 0 | 2 | 102 | **0%** | 0% |
| tier 2 (octokit common) | 129 | 0 | 0 | 6 | 123 | **0%** | 0% |
| tier 3 (rest) | 872 | 0 | 0 | 15 | 857 | **0%** | 0% |

## Per tag

| tag | ops | conforming | nonconforming | not impl. | untested | % conforming | % of tested |
|---|---:|---:|---:|---:|---:|---:|---:|
| repos | 193 | 0 | 0 | 1 | 192 | **0%** | 0% |
| enterprise-admin | 171 | 0 | 0 | 0 | 171 | **0%** | – |
| actions | 160 | 0 | 0 | 6 | 154 | **0%** | 0% |
| orgs | 89 | 0 | 0 | 0 | 89 | **0%** | – |
| issues | 47 | 0 | 0 | 0 | 47 | **0%** | – |
| apps | 39 | 0 | 0 | 0 | 39 | **0%** | – |
| users | 37 | 0 | 0 | 1 | 36 | **0%** | 0% |
| teams | 35 | 0 | 0 | 0 | 35 | **0%** | – |
| activity | 32 | 0 | 0 | 0 | 32 | **0%** | – |
| dependabot | 28 | 0 | 0 | 0 | 28 | **0%** | – |
| packages | 27 | 0 | 0 | 0 | 27 | **0%** | – |
| pulls | 27 | 0 | 0 | 0 | 27 | **0%** | – |
| projects | 26 | 0 | 0 | 0 | 26 | **0%** | – |
| secret-scanning | 23 | 0 | 0 | 4 | 19 | **0%** | 0% |
| code-security | 20 | 0 | 0 | 0 | 20 | **0%** | – |
| gists | 20 | 0 | 0 | 0 | 20 | **0%** | – |
| code-scanning | 17 | 0 | 0 | 0 | 17 | **0%** | – |
| reactions | 15 | 0 | 0 | 0 | 15 | **0%** | – |
| git | 13 | 0 | 0 | 0 | 13 | **0%** | – |
| checks | 12 | 0 | 0 | 0 | 12 | **0%** | – |
| migrations | 11 | 0 | 0 | 0 | 11 | **0%** | – |
| oauth-authorizations | 10 | 0 | 0 | 0 | 10 | **0%** | – |
| search | 7 | 0 | 0 | 0 | 7 | **0%** | – |
| enterprise-team-organizations | 6 | 0 | 0 | 0 | 6 | **0%** | – |
| private-registries | 6 | 0 | 0 | 0 | 6 | **0%** | – |
| enterprise-teams | 5 | 0 | 0 | 0 | 5 | **0%** | – |
| enterprise-team-memberships | 5 | 0 | 0 | 0 | 5 | **0%** | – |
| meta | 4 | 0 | 0 | 2 | 2 | **0%** | 0% |
| licenses | 3 | 0 | 0 | 2 | 1 | **0%** | 0% |
| dependency-graph | 3 | 0 | 0 | 0 | 3 | **0%** | – |
| security-advisories | 2 | 0 | 0 | 0 | 2 | **0%** | – |
| codes-of-conduct | 2 | 0 | 0 | 0 | 2 | **0%** | – |
| billing | 2 | 0 | 0 | 0 | 2 | **0%** | – |
| gitignore | 2 | 0 | 0 | 2 | 0 | **0%** | 0% |
| markdown | 2 | 0 | 0 | 2 | 0 | **0%** | 0% |
| oidc | 2 | 0 | 0 | 2 | 0 | **0%** | 0% |
| emojis | 1 | 0 | 0 | 1 | 0 | **0%** | 0% |
| rate-limit | 1 | 0 | 0 | 0 | 1 | **0%** | – |

## Failing operations (23)

- **repos/create-in-org** (tier 1, `POST /orgs/{org}/repos`): not_implemented
  - `404 for a route that should exist (expected 201)`
- **users/get-authenticated** (tier 1, `GET /user`): not_implemented
  - `404 for a route that should exist (expected 200)`
- **emojis/get** (tier 2, `GET /emojis`): not_implemented
  - `404 for a route that should exist (expected 200)`
- **gitignore/get-all-templates** (tier 2, `GET /gitignore/templates`): not_implemented
  - `404 for a route that should exist (expected 200)`
- **licenses/get-all-commonly-used** (tier 2, `GET /licenses`): not_implemented
  - `404 for a route that should exist (expected 200)`
- **markdown/render** (tier 2, `POST /markdown`): not_implemented
  - `404 for a route that should exist (expected 200)`
- **meta/get-octocat** (tier 2, `GET /octocat`): not_implemented
  - `404 for a route that should exist (expected 200)`
- **meta/get-zen** (tier 2, `GET /zen`): not_implemented
  - `404 for a route that should exist (expected 200)`
- **secret-scanning/list-enterprise-pattern-configs** (tier 3, `GET /enterprises/{enterprise}/secret-scanning/pattern-configurations`): not_implemented
  - `404 for a route that should exist (expected 200)`
- **secret-scanning/update-enterprise-pattern-configs** (tier 3, `PATCH /enterprises/{enterprise}/secret-scanning/pattern-configurations`): not_implemented
  - `404 for a route that should exist (expected 200)`
- **gitignore/get-template** (tier 3, `GET /gitignore/templates/{name}`): not_implemented
  - `404 for a route that should exist (expected 200)`
- **licenses/get** (tier 3, `GET /licenses/{license}`): not_implemented
  - `404 for a route that should exist (expected 200)`
- **markdown/render-raw** (tier 3, `POST /markdown/raw`): not_implemented
  - `404 for a route that should exist (expected 200)`
- **actions/get-actions-cache-usage-for-org** (tier 3, `GET /orgs/{org}/actions/cache/usage`): not_implemented
  - `404 for a route that should exist (expected 200)`
- **actions/get-actions-cache-usage-by-repo-for-org** (tier 3, `GET /orgs/{org}/actions/cache/usage-by-repository`): not_implemented
  - `404 for a route that should exist (expected 200)`
- **oidc/get-oidc-custom-sub-template-for-org** (tier 3, `GET /orgs/{org}/actions/oidc/customization/sub`): not_implemented
  - `404 for a route that should exist (expected 200)`
- **oidc/update-oidc-custom-sub-template-for-org** (tier 3, `PUT /orgs/{org}/actions/oidc/customization/sub`): not_implemented
  - `404 for a route that should exist (expected 201)`
- **actions/list-runner-applications-for-org** (tier 3, `GET /orgs/{org}/actions/runners/downloads`): not_implemented
  - `404 for a route that should exist (expected 200)`
- **actions/generate-runner-jitconfig-for-org** (tier 3, `POST /orgs/{org}/actions/runners/generate-jitconfig`): not_implemented
  - `404 for a route that should exist (expected 201)`
- **actions/create-registration-token-for-org** (tier 3, `POST /orgs/{org}/actions/runners/registration-token`): not_implemented
  - `404 for a route that should exist (expected 201)`
- **actions/create-remove-token-for-org** (tier 3, `POST /orgs/{org}/actions/runners/remove-token`): not_implemented
  - `404 for a route that should exist (expected 201)`
- **secret-scanning/list-org-pattern-configs** (tier 3, `GET /orgs/{org}/secret-scanning/pattern-configurations`): not_implemented
  - `404 for a route that should exist (expected 200)`
- **secret-scanning/update-org-pattern-configs** (tier 3, `PATCH /orgs/{org}/secret-scanning/pattern-configurations`): not_implemented
  - `404 for a route that should exist (expected 200)`

## Conforming operations (0)

_none_

## Untested tier 1/2 operations (225)

- `meta/root` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `gists/list` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `gists/create` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `gists/get` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `gists/update` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `gists/delete` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `issues/list` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `meta/get` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `activity/list-notifications-for-authenticated-user` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `orgs/list` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `orgs/get` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `orgs/list-members` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `orgs/check-membership-for-user` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `repos/list-for-org` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `teams/list` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `rate-limit/get` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `repos/get` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `repos/update` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `repos/delete` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `actions/list-workflow-runs-for-repo` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `actions/get-workflow-run` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `actions/list-jobs-for-workflow-run` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `actions/list-repo-workflows` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `issues/list-assignees` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `issues/check-user-can-be-assigned` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `repos/list-branches` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `repos/get-branch` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `repos/get-branch-protection` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `repos/update-branch-protection` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `repos/delete-branch-protection` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `repos/rename-branch` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `checks/get` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `checks/list-annotations` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `checks/get-suite` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `repos/list-collaborators` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `repos/check-collaborator` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `repos/add-collaborator` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `repos/remove-collaborator` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `repos/get-collaborator-permission-level` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `repos/list-commit-comments-for-repo` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `repos/get-commit-comment` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `repos/update-commit-comment` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `repos/delete-commit-comment` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `reactions/list-for-commit-comment` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `reactions/create-for-commit-comment` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `reactions/delete-for-commit-comment` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `repos/list-commits` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `repos/list-branches-for-head-commit` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `repos/list-comments-for-commit` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `repos/create-commit-comment` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `repos/list-pull-requests-associated-with-commit` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `repos/get-commit` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `checks/list-for-ref` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `checks/list-suites-for-ref` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `repos/get-combined-status-for-ref` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `repos/list-commit-statuses-for-ref` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `repos/compare-commits` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `repos/get-content` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `repos/create-or-update-file-contents` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `repos/delete-file` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `repos/list-contributors` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `repos/list-deployments` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `repos/create-deployment` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `repos/get-deployment` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `repos/delete-deployment` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `repos/list-deployment-statuses` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `repos/create-deployment-status` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `activity/list-repo-events` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `repos/list-forks` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `repos/create-fork` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `git/create-blob` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `git/get-blob` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `git/create-commit` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `git/get-commit` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `git/list-matching-refs` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `git/get-ref` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `git/create-ref` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `git/update-ref` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `git/delete-ref` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `git/create-tag` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `git/get-tag` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `git/create-tree` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `git/get-tree` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `repos/list-webhooks` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `repos/create-webhook` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `repos/get-webhook` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `repos/update-webhook` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `repos/delete-webhook` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `repos/ping-webhook` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `repos/list-invitations` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `issues/list-for-repo` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `issues/create` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `issues/list-comments-for-repo` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `issues/get-comment` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `issues/update-comment` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `issues/delete-comment` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `reactions/list-for-issue-comment` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `reactions/create-for-issue-comment` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `reactions/delete-for-issue-comment` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `issues/list-events-for-repo` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `issues/get-event` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `issues/get` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `issues/update` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `issues/add-assignees` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `issues/remove-assignees` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `issues/list-comments` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `issues/create-comment` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `issues/list-events` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `issues/list-labels-on-issue` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `issues/set-labels` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `issues/add-labels` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `issues/remove-all-labels` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `issues/remove-label` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `issues/lock` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `issues/unlock` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `reactions/list-for-issue` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `reactions/create-for-issue` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `reactions/delete-for-issue` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `issues/list-events-for-timeline` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `issues/list-labels-for-repo` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `issues/create-label` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `issues/get-label` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `issues/update-label` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `issues/delete-label` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `repos/list-languages` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `licenses/get-for-repo` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `repos/merge-upstream` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `repos/merge` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `issues/list-milestones` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `issues/create-milestone` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `issues/get-milestone` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `issues/update-milestone` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `issues/delete-milestone` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `pulls/list` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `pulls/create` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `pulls/list-review-comments-for-repo` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `pulls/get-review-comment` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `pulls/update-review-comment` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `pulls/delete-review-comment` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `reactions/list-for-pull-request-review-comment` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `reactions/create-for-pull-request-review-comment` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `reactions/delete-for-pull-request-comment` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `pulls/get` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `pulls/update` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `pulls/list-review-comments` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `pulls/create-review-comment` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `pulls/create-reply-for-review-comment` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `pulls/list-commits` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `pulls/list-files` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `pulls/check-if-merged` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `pulls/merge` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `pulls/list-requested-reviewers` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `pulls/request-reviewers` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `pulls/remove-requested-reviewers` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `pulls/list-reviews` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `pulls/create-review` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `pulls/get-review` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `pulls/update-review` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `pulls/delete-pending-review` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `pulls/list-comments-for-review` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `pulls/dismiss-review` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `pulls/submit-review` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `pulls/update-branch` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `repos/get-readme` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `repos/list-releases` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `repos/create-release` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `repos/get-release-asset` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `repos/update-release-asset` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `repos/delete-release-asset` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `repos/generate-release-notes` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `repos/get-latest-release` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `repos/get-release-by-tag` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `repos/get-release` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `repos/update-release` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `repos/delete-release` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `repos/list-release-assets` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `repos/upload-release-asset` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `reactions/list-for-release` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `reactions/create-for-release` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `reactions/delete-for-release` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `activity/list-stargazers-for-repo` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `repos/create-commit-status` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `activity/list-watchers-for-repo` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `activity/get-repo-subscription` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `activity/set-repo-subscription` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `activity/delete-repo-subscription` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `repos/list-tags` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `repos/download-tarball-archive` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `repos/get-all-topics` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `repos/replace-all-topics` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `repos/download-zipball-archive` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `repos/create-using-template` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `search/code` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `search/commits` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `search/issues-and-pull-requests` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `search/labels` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `search/repos` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `search/topics` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `search/users` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `users/update-authenticated` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `users/list-emails-for-authenticated-user` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `users/list-gpg-keys-for-authenticated-user` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `users/create-gpg-key-for-authenticated-user` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `users/delete-gpg-key-for-authenticated-user` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `issues/list-for-authenticated-user` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `users/list-public-ssh-keys-for-authenticated-user` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `users/create-public-ssh-key-for-authenticated-user` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `users/delete-public-ssh-key-for-authenticated-user` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `orgs/get-membership-for-authenticated-user` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `orgs/list-for-authenticated-user` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `repos/list-for-authenticated-user` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `repos/create-for-authenticated-user` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `activity/list-repos-starred-by-authenticated-user` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `activity/check-repo-is-starred-by-authenticated-user` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `activity/star-repo-for-authenticated-user` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `activity/unstar-repo-for-authenticated-user` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `activity/list-watched-repos-for-authenticated-user` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `users/list` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `users/get-by-username` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `users/list-followers-for-user` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `users/list-following-for-user` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `users/list-public-keys-for-user` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `orgs/list-for-user` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `repos/list-for-user` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `activity/list-repos-starred-by-user` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)

## Scenarios

- blocked: identity & meta — fixture step users/get-authenticated returned 404: Not found.

- skipped: repository: create (fixture) — requires login
- skipped: repository: metadata — requires rp
- skipped: contents API — requires rp
- skipped: git data & refs — requires rp, baseSha
- skipped: branches, merges, protection, archives — requires rp, featureSha
- skipped: commits, compare, statuses, checks, commit comments — requires rp, featureSha
- skipped: labels & milestones — requires rp
- skipped: issues — requires rp, login, label, milestone
- skipped: issue comments & reactions — requires rp, issue
- skipped: pull requests: create, review comments, reviews — requires rp, featureSha
- skipped: pull requests: update-branch & close — requires rp, baseSha
- skipped: pull requests: merge — requires rp, pr
- skipped: releases & assets — requires rp
- skipped: deployments & webhooks — requires rp
- skipped: stars, watching, notifications, events — requires rp, login
- skipped: collaborators — requires rp, login
- skipped: template & fork — requires rp, login
- skipped: users & ssh keys — requires login
- skipped: organizations — requires login
- skipped: gists — requires login
- skipped: actions (read) — requires rp
- skipped: search — requires rp, login
- completed: misc endpoints
- skipped: actions-settings: repository secrets and variables — requires rp
- blocked: actions-settings: org secrets and variables with selected repositories — fixture step repos/create-in-org returned 404: Not found.

- skipped: actions-settings: repository permissions and OIDC — requires rp
- skipped: actions-settings: organization permissions — requires actsetRepo
- skipped: actions-settings: organization runner groups — requires actsetRepo
- skipped: actions-settings: enterprise permissions and runner groups — requires me
- skipped: advsec: global advisories — requires login
- blocked: advsec: code security configuration on an org repository — fixture step repos/create-in-org returned 404: Not found.

- skipped: advsec: secret scanning, push protection and bypasses — requires advsec
- completed: advsec: secret scanning pattern configurations
- skipped: advsec: Dependabot alerts and dismissal requests — requires advsec
- skipped: advsec: Dependabot secrets and repository access — requires advsec
- skipped: advsec: code scanning (SARIF upload, analyses, alerts) — requires advsec
- skipped: advsec: detach and delete the org configuration — requires advsec
- skipped: advsec: enterprise code security configurations — requires me
- skipped: apps: GitHub App, JWT, installations, installation tokens, webhook — requires login, rp, repoId
- skipped: apps: enterprise app installations on an organization — requires login
- skipped: apps: manifest flow — requires login
- skipped: oauth-authorizations: authorizations and grants — requires login
- skipped: migrations: user export — requires login, rp
- skipped: migrations: organization export with locked repositories — requires login
- skipped: checks: runs, annotations, suites — requires rp, featureSha
- skipped: checks: gpg keys — requires login
- skipped: checks: review dismissal, PR check runs — requires rp, baseSha
- skipped: checks: workflow run fixture — requires rp
- skipped: enterprise: users, impersonation, suspension — requires login
- skipped: enterprise: global webhooks — requires login
- skipped: enterprise: stats, license, announcement, keys — requires login
- skipped: enterprise: organizations, LDAP team mapping — requires login
- skipped: enterprise: pre-receive environments and hooks — requires login, rp, entOrg
- skipped: enterprise: self-hosted runners — requires login
- skipped: enterprise: audit log, streams, code security, billing, rulesets, bypass requests — requires login, entOrg
- skipped: enterprise: custom properties — requires login, entOrg
- skipped: enterprise: SCIM provisioning, enterprise teams, external groups — requires login, entOrg, entOrgTeam, entUser
- skipped: enterprise: live migrations — requires login, rp
- skipped: enterprise: Manage API (nodes, settings, maintenance, license, ssh, upgrades) — requires login
- skipped: enterprise: cleanup — requires login
- completed: finish: gitignore template, license, raw markdown
- skipped: finish: repository invitations — requires login
- skipped: finish: enterprise runner group membership — requires login
- skipped: finish: push ruleset delegated bypass requests — requires rp
- skipped: orgs: settings, announcement, immutable releases, security products — requires login
- skipped: orgs: public membership — requires login
- skipped: teams: nested teams, members, repositories (slug and legacy routes) — requires login
- skipped: orgs: custom repository roles — requires login
- skipped: orgs: organization roles and security managers — requires login
- skipped: orgs: outside collaborators — requires login
- skipped: orgs: custom properties — requires login
- skipped: private registries — requires login
- skipped: orgs: ruleset history, fine-grained PAT policy, audit log — requires login
- skipped: orgs: webhooks, memberships, team membership — requires login
- skipped: orgs: delete organization — requires login
- skipped: packages: container images and npm packages (user namespace) — requires login
- skipped: packages: organization packages — requires login
- skipped: projects: Projects (v2) REST for the authenticated user — requires login, me
- skipped: projects: Projects (v2) REST for an organization — requires login
- skipped: repos: environments, branch policies, deployment statuses — requires rp, me
- skipped: repos: security & feature toggles — requires rp
- skipped: repos: properties, dispatch, CODEOWNERS, teams, caches, public list — requires rp, login
- skipped: repos: statistics — requires rp
- skipped: repos: branch protection sub-resources — requires rp, baseSha, login
- skipped: repos: rulesets, history, rule suites — requires rp
- skipped: repos: pages (legacy builds and artifact deployments) — requires rp, def
- skipped: repos: webhooks extras, deploy keys, autolinks, transfer — requires rp, login
- skipped: users: emails, social accounts, follows, signing keys, hovercard — requires login, me
- skipped: issues: pinned comments, dependencies, field values — requires rp, login
- skipped: gists: comments, stars, forks, revisions — requires login
- skipped: activity: notifications, feeds, events, codes of conduct — requires rp, login
- skipped: dependency graph: SBOM, compare, snapshots — requires rp, def, baseSha
- skipped: runner: fixture repository and runner process — requires login
- skipped: runner: push workflow, runs, jobs, logs, artifacts, caches, re-runs — requires runnerRepo, runnerReady
- skipped: runner: dispatch, cancel, force-cancel — requires runnerRepo, runnerReady
- skipped: runner: environment reviews and deployment protection rules — requires runnerRepo, runnerReady, me
- blocked: runner: organization runners, OIDC, cache usage — fixture step actions/generate-runner-jitconfig-for-org returned 404: Not found.

- skipped: runner: enterprise runners and cache policy — requires login
- completed: runner: cleanup
- skipped: cleanup — requires rp

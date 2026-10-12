# REST conformance report

- **Target:** `http://localhost:3000/api/v3`
- **Spec:** `descriptions/ghes-3.22/ghes-3.22.json` @ [`58b1e0c00b`](https://github.com/github/rest-api-description/tree/58b1e0c00b39b9e46c24dc4a0ba6a2669c90e9c1) (API 1.1.4)
- **Run:** 2026-10-12T21:42:03.195Z · 86 HTTP requests · max tier 3

## Score

| scope | ops | conforming | nonconforming | not impl. | untested | % conforming | % of tested |
|---|---:|---:|---:|---:|---:|---:|---:|
| **all operations** | 1105 | 20 | 0 | 54 | 1031 | **1.8%** | 27% |
| tier 1 (gh CLI core) | 104 | 14 | 0 | 16 | 74 | **13.5%** | 46.7% |
| tier 2 (octokit common) | 129 | 6 | 0 | 38 | 85 | **4.7%** | 13.6% |
| tier 3 (rest) | 872 | 0 | 0 | 0 | 872 | **0%** | – |

## Per tag

| tag | ops | conforming | nonconforming | not impl. | untested | % conforming | % of tested |
|---|---:|---:|---:|---:|---:|---:|---:|
| repos | 193 | 8 | 0 | 16 | 169 | **4.1%** | 33.3% |
| enterprise-admin | 171 | 0 | 0 | 0 | 171 | **0%** | – |
| actions | 160 | 0 | 0 | 2 | 158 | **0%** | 0% |
| orgs | 89 | 2 | 0 | 1 | 86 | **2.2%** | 66.7% |
| issues | 47 | 0 | 0 | 1 | 46 | **0%** | 0% |
| apps | 39 | 0 | 0 | 0 | 39 | **0%** | – |
| users | 37 | 5 | 0 | 6 | 26 | **13.5%** | 45.5% |
| teams | 35 | 0 | 0 | 0 | 35 | **0%** | – |
| activity | 32 | 0 | 0 | 13 | 19 | **0%** | 0% |
| dependabot | 28 | 0 | 0 | 0 | 28 | **0%** | – |
| packages | 27 | 0 | 0 | 0 | 27 | **0%** | – |
| pulls | 27 | 0 | 0 | 0 | 27 | **0%** | – |
| projects | 26 | 0 | 0 | 0 | 26 | **0%** | – |
| secret-scanning | 23 | 0 | 0 | 0 | 23 | **0%** | – |
| code-security | 20 | 0 | 0 | 0 | 20 | **0%** | – |
| gists | 20 | 0 | 0 | 2 | 18 | **0%** | 0% |
| code-scanning | 17 | 0 | 0 | 0 | 17 | **0%** | – |
| reactions | 15 | 0 | 0 | 0 | 15 | **0%** | – |
| git | 13 | 0 | 0 | 1 | 12 | **0%** | 0% |
| checks | 12 | 0 | 0 | 0 | 12 | **0%** | – |
| migrations | 11 | 0 | 0 | 0 | 11 | **0%** | – |
| oauth-authorizations | 10 | 0 | 0 | 0 | 10 | **0%** | – |
| search | 7 | 0 | 0 | 7 | 0 | **0%** | 0% |
| enterprise-team-organizations | 6 | 0 | 0 | 0 | 6 | **0%** | – |
| private-registries | 6 | 0 | 0 | 0 | 6 | **0%** | – |
| enterprise-teams | 5 | 0 | 0 | 0 | 5 | **0%** | – |
| enterprise-team-memberships | 5 | 0 | 0 | 0 | 5 | **0%** | – |
| meta | 4 | 4 | 0 | 0 | 0 | **100%** | 100% |
| licenses | 3 | 0 | 0 | 2 | 1 | **0%** | 0% |
| dependency-graph | 3 | 0 | 0 | 0 | 3 | **0%** | – |
| security-advisories | 2 | 0 | 0 | 0 | 2 | **0%** | – |
| codes-of-conduct | 2 | 0 | 0 | 0 | 2 | **0%** | – |
| billing | 2 | 0 | 0 | 0 | 2 | **0%** | – |
| gitignore | 2 | 0 | 0 | 1 | 1 | **0%** | 0% |
| markdown | 2 | 0 | 0 | 1 | 1 | **0%** | 0% |
| oidc | 2 | 0 | 0 | 0 | 2 | **0%** | – |
| emojis | 1 | 0 | 0 | 1 | 0 | **0%** | 0% |
| rate-limit | 1 | 1 | 0 | 0 | 0 | **100%** | 100% |

## Failing operations (54)

- **repos/get-content** (tier 1, `GET /repos/{owner}/{repo}/contents/{path}`): not_implemented
  - `404 for a route that should exist (expected 200)`
- **repos/create-or-update-file-contents** (tier 1, `PUT /repos/{owner}/{repo}/contents/{path}`): not_implemented
  - `404 for a route that should exist (expected 201)`
- **repos/list-forks** (tier 1, `GET /repos/{owner}/{repo}/forks`): not_implemented
  - `404 for a route that should exist (expected 200)`
- **git/get-ref** (tier 1, `GET /repos/{owner}/{repo}/git/ref/{ref}`): not_implemented
  - `404 for a route that should exist (expected 200)`
- **issues/create-label** (tier 1, `POST /repos/{owner}/{repo}/labels`): not_implemented
  - `404 for a route that should exist (expected 201)`
- **repos/get-readme** (tier 1, `GET /repos/{owner}/{repo}/readme`): not_implemented
  - `404 for a route that should exist (expected 200)`
- **repos/create-release** (tier 1, `POST /repos/{owner}/{repo}/releases`): not_implemented
  - `404 for a route that should exist (expected 201)`
- **repos/create-using-template** (tier 1, `POST /repos/{template_owner}/{template_repo}/generate`): not_implemented
  - `404 for a route that should exist (expected 201)`
- **search/code** (tier 1, `GET /search/code`): not_implemented
  - `404 for a route that should exist (expected 200)`
- **search/commits** (tier 1, `GET /search/commits`): not_implemented
  - `404 for a route that should exist (expected 200)`
- **search/issues-and-pull-requests** (tier 1, `GET /search/issues`): not_implemented
  - `404 for a route that should exist (expected 200)`
- **search/repos** (tier 1, `GET /search/repositories`): not_implemented
  - `404 for a route that should exist (expected 200)`
- **search/users** (tier 1, `GET /search/users`): not_implemented
  - `404 for a route that should exist (expected 200)`
- **users/list-gpg-keys-for-authenticated-user** (tier 1, `GET /user/gpg_keys`): not_implemented
  - `404 for a route that should exist (expected 200)`
- **users/list-public-ssh-keys-for-authenticated-user** (tier 1, `GET /user/keys`): not_implemented
  - `404 for a route that should exist (expected 200)`
- **users/create-public-ssh-key-for-authenticated-user** (tier 1, `POST /user/keys`): not_implemented
  - `404 for a route that should exist (expected 201)`
- **emojis/get** (tier 2, `GET /emojis`): not_implemented
  - `404 for a route that should exist (expected 200)`
- **gists/list** (tier 2, `GET /gists`): not_implemented
  - `404 for a route that should exist (expected 200)`
- **gists/create** (tier 2, `POST /gists`): not_implemented
  - `404 for a route that should exist (expected 201)`
- **gitignore/get-all-templates** (tier 2, `GET /gitignore/templates`): not_implemented
  - `404 for a route that should exist (expected 200)`
- **licenses/get-all-commonly-used** (tier 2, `GET /licenses`): not_implemented
  - `404 for a route that should exist (expected 200)`
- **markdown/render** (tier 2, `POST /markdown`): not_implemented
  - `404 for a route that should exist (expected 200)`
- **activity/list-notifications-for-authenticated-user** (tier 2, `GET /notifications`): not_implemented
  - `404 for a route that should exist (expected 200)`
- **orgs/list** (tier 2, `GET /organizations`): not_implemented
  - `404 for a route that should exist (expected 200)`
- **actions/list-workflow-runs-for-repo** (tier 2, `GET /repos/{owner}/{repo}/actions/runs`): not_implemented
  - `404 for a route that should exist (expected 200)`
- **actions/list-repo-workflows** (tier 2, `GET /repos/{owner}/{repo}/actions/workflows`): not_implemented
  - `404 for a route that should exist (expected 200)`
- **repos/list-collaborators** (tier 2, `GET /repos/{owner}/{repo}/collaborators`): not_implemented
  - `404 for a route that should exist (expected 200)`
- **repos/check-collaborator** (tier 2, `GET /repos/{owner}/{repo}/collaborators/{username}`): not_implemented
  - `404 for a route that should exist (expected 204)`
- **repos/get-collaborator-permission-level** (tier 2, `GET /repos/{owner}/{repo}/collaborators/{username}/permission`): not_implemented
  - `404 for a route that should exist (expected 200)`
- **repos/list-contributors** (tier 2, `GET /repos/{owner}/{repo}/contributors`): not_implemented
  - `404 for a route that should exist (expected 200/204)`
- **repos/list-deployments** (tier 2, `GET /repos/{owner}/{repo}/deployments`): not_implemented
  - `404 for a route that should exist (expected 200)`
- **repos/create-deployment** (tier 2, `POST /repos/{owner}/{repo}/deployments`): not_implemented
  - `404 for a route that should exist (expected 201)`
- **activity/list-repo-events** (tier 2, `GET /repos/{owner}/{repo}/events`): not_implemented
  - `404 for a route that should exist (expected 200)`
- **repos/list-webhooks** (tier 2, `GET /repos/{owner}/{repo}/hooks`): not_implemented
  - `404 for a route that should exist (expected 200)`
- **repos/create-webhook** (tier 2, `POST /repos/{owner}/{repo}/hooks`): not_implemented
  - `404 for a route that should exist (expected 201)`
- **repos/list-invitations** (tier 2, `GET /repos/{owner}/{repo}/invitations`): not_implemented
  - `404 for a route that should exist (expected 200)`
- **licenses/get-for-repo** (tier 2, `GET /repos/{owner}/{repo}/license`): not_implemented
  - `404 for a route that should exist (expected 200)`
- **activity/list-stargazers-for-repo** (tier 2, `GET /repos/{owner}/{repo}/stargazers`): not_implemented
  - `404 for a route that should exist (expected 200)`
- **activity/list-watchers-for-repo** (tier 2, `GET /repos/{owner}/{repo}/subscribers`): not_implemented
  - `404 for a route that should exist (expected 200)`
- **activity/get-repo-subscription** (tier 2, `GET /repos/{owner}/{repo}/subscription`): not_implemented
  - `404 for a route that should exist (expected 200)`
- **activity/set-repo-subscription** (tier 2, `PUT /repos/{owner}/{repo}/subscription`): not_implemented
  - `404 for a route that should exist (expected 200)`
- **activity/delete-repo-subscription** (tier 2, `DELETE /repos/{owner}/{repo}/subscription`): not_implemented
  - `404 for a route that should exist (expected 204)`
- **repos/list-tags** (tier 2, `GET /repos/{owner}/{repo}/tags`): not_implemented
  - `404 for a route that should exist (expected 200)`
- **search/labels** (tier 2, `GET /search/labels`): not_implemented
  - `404 for a route that should exist (expected 200)`
- **search/topics** (tier 2, `GET /search/topics`): not_implemented
  - `404 for a route that should exist (expected 200)`
- **activity/list-repos-starred-by-authenticated-user** (tier 2, `GET /user/starred`): not_implemented
  - `404 for a route that should exist (expected 200)`
- **activity/check-repo-is-starred-by-authenticated-user** (tier 2, `GET /user/starred/{owner}/{repo}`): not_implemented
  - `404 for a route that should exist (expected 204)`
- **activity/star-repo-for-authenticated-user** (tier 2, `PUT /user/starred/{owner}/{repo}`): not_implemented
  - `404 for a route that should exist (expected 204)`
- **activity/unstar-repo-for-authenticated-user** (tier 2, `DELETE /user/starred/{owner}/{repo}`): not_implemented
  - `404 for a route that should exist (expected 204)`
- **activity/list-watched-repos-for-authenticated-user** (tier 2, `GET /user/subscriptions`): not_implemented
  - `404 for a route that should exist (expected 200)`
- **users/list-followers-for-user** (tier 2, `GET /users/{username}/followers`): not_implemented
  - `404 for a route that should exist (expected 200)`
- **users/list-following-for-user** (tier 2, `GET /users/{username}/following`): not_implemented
  - `404 for a route that should exist (expected 200)`
- **users/list-public-keys-for-user** (tier 2, `GET /users/{username}/keys`): not_implemented
  - `404 for a route that should exist (expected 200)`
- **activity/list-repos-starred-by-user** (tier 2, `GET /users/{username}/starred`): not_implemented
  - `404 for a route that should exist (expected 200)`

## Conforming operations (20)

`meta/root`, `meta/get`, `meta/get-octocat`, `rate-limit/get`, `repos/update`, `repos/delete`, `repos/list-languages`, `repos/get-all-topics`, `repos/replace-all-topics`, `users/get-authenticated`, `users/update-authenticated`, `users/list-emails-for-authenticated-user`, `orgs/list-for-authenticated-user`, `repos/list-for-authenticated-user`, `repos/create-for-authenticated-user`, `users/list`, `users/get-by-username`, `orgs/list-for-user`, `repos/list-for-user`, `meta/get-zen`

## Untested tier 1/2 operations (159)

- `gists/get` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `gists/update` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `gists/delete` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `issues/list` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `orgs/get` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `orgs/list-members` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `orgs/check-membership-for-user` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `repos/list-for-org` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `repos/create-in-org` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `teams/list` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `repos/get` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `actions/get-workflow-run` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `actions/list-jobs-for-workflow-run` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `issues/list-assignees` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `issues/check-user-can-be-assigned` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `repos/list-branches` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `repos/get-branch` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `repos/get-branch-protection` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `repos/update-branch-protection` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `repos/delete-branch-protection` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `repos/rename-branch` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `checks/get` (tier 2): no scenario
- `checks/list-annotations` (tier 2): no scenario
- `checks/get-suite` (tier 2): no scenario
- `repos/add-collaborator` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `repos/remove-collaborator` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
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
- `repos/delete-file` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `repos/get-deployment` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `repos/delete-deployment` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `repos/list-deployment-statuses` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `repos/create-deployment-status` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `repos/create-fork` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `git/create-blob` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `git/get-blob` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `git/create-commit` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `git/get-commit` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `git/list-matching-refs` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `git/create-ref` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `git/update-ref` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `git/delete-ref` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `git/create-tag` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `git/get-tag` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `git/create-tree` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `git/get-tree` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `repos/get-webhook` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `repos/update-webhook` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `repos/delete-webhook` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `repos/ping-webhook` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
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
- `issues/get-label` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `issues/update-label` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `issues/delete-label` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
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
- `pulls/dismiss-review` (tier 2): no scenario
- `pulls/submit-review` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `pulls/update-branch` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `repos/list-releases` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
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
- `repos/create-commit-status` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `repos/download-tarball-archive` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `repos/download-zipball-archive` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `users/create-gpg-key-for-authenticated-user` (tier 1): no scenario
- `users/delete-gpg-key-for-authenticated-user` (tier 1): no scenario
- `issues/list-for-authenticated-user` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `users/delete-public-ssh-key-for-authenticated-user` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `orgs/get-membership-for-authenticated-user` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)

## Scenarios

- completed: identity & meta
- blocked: repository: create (fixture) — could not determine the head commit of the default branch
- completed: repository: metadata
- blocked: contents API — fixture step repos/create-or-update-file-contents returned 404: {"documentation_url":"https://docs.github.com/rest","message":"Not Found","status":"404"}
- skipped: git data & refs — requires baseSha
- skipped: branches, merges, protection, archives — requires featureSha
- skipped: commits, compare, statuses, checks, commit comments — requires featureSha
- blocked: labels & milestones — fixture step issues/create-label returned 404: {"documentation_url":"https://docs.github.com/rest","message":"Not Found","status":"404"}
- skipped: issues — requires label, milestone
- skipped: issue comments & reactions — requires issue
- skipped: pull requests: create, review comments, reviews — requires featureSha
- skipped: pull requests: update-branch & close — requires baseSha
- skipped: pull requests: merge — requires pr
- blocked: releases & assets — fixture step repos/create-release returned 404: {"documentation_url":"https://docs.github.com/rest","message":"Not Found","status":"404"}
- completed: deployments & webhooks
- completed: stars, watching, notifications, events
- completed: collaborators
- completed: template & fork
- completed: users & ssh keys
- completed: organizations
- completed: gists
- completed: actions (read)
- completed: search
- completed: misc endpoints
- completed: cleanup

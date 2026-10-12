# REST conformance report

- **Target:** `https://api.github.com` (read-only scenarios)
- **Spec:** `descriptions/api.github.com/api.github.com.json` @ [`58b1e0c00b`](https://github.com/github/rest-api-description/tree/58b1e0c00b39b9e46c24dc4a0ba6a2669c90e9c1) (API 1.1.4)
- **Run:** 2026-10-12T21:35:49.580Z · 27 HTTP requests · max tier 3

## Score

| scope | ops | conforming | nonconforming | not impl. | untested | % conforming | % of tested |
|---|---:|---:|---:|---:|---:|---:|---:|
| **all operations** | 1232 | 24 | 0 | 0 | 1208 | **1.9%** | 100% |
| tier 1 (gh CLI core) | 104 | 20 | 0 | 0 | 84 | **19.2%** | 100% |
| tier 2 (octokit common) | 129 | 4 | 0 | 0 | 125 | **3.1%** | 100% |
| tier 3 (rest) | 999 | 0 | 0 | 0 | 999 | **0%** | 0% |

## Per tag

| tag | ops | conforming | nonconforming | not impl. | untested | % conforming | % of tested |
|---|---:|---:|---:|---:|---:|---:|---:|
| repos | 203 | 10 | 0 | 0 | 193 | **4.9%** | 100% |
| actions | 200 | 0 | 0 | 0 | 200 | **0%** | 0% |
| orgs | 116 | 0 | 0 | 0 | 116 | **0%** | 0% |
| issues | 61 | 5 | 0 | 0 | 56 | **8.2%** | 100% |
| codespaces | 48 | 0 | 0 | 0 | 48 | **0%** | 0% |
| users | 47 | 1 | 0 | 0 | 46 | **2.1%** | 100% |
| apps | 37 | 0 | 0 | 0 | 37 | **0%** | 0% |
| pulls | 35 | 3 | 0 | 0 | 32 | **8.6%** | 100% |
| activity | 34 | 0 | 0 | 0 | 34 | **0%** | 0% |
| teams | 32 | 0 | 0 | 0 | 32 | **0%** | 0% |
| copilot | 31 | 0 | 0 | 0 | 31 | **0%** | 0% |
| agents | 30 | 0 | 0 | 0 | 30 | **0%** | 0% |
| copilot-spaces | 28 | 0 | 0 | 0 | 28 | **0%** | 0% |
| packages | 27 | 0 | 0 | 0 | 27 | **0%** | 0% |
| projects | 26 | 0 | 0 | 0 | 26 | **0%** | 0% |
| dependabot | 25 | 0 | 0 | 0 | 25 | **0%** | 0% |
| code-scanning | 25 | 0 | 0 | 0 | 25 | **0%** | 0% |
| migrations | 22 | 0 | 0 | 0 | 22 | **0%** | 0% |
| code-security | 20 | 0 | 0 | 0 | 20 | **0%** | 0% |
| gists | 20 | 0 | 0 | 0 | 20 | **0%** | 0% |
| secret-scanning | 17 | 0 | 0 | 0 | 17 | **0%** | 0% |
| interactions | 16 | 0 | 0 | 0 | 16 | **0%** | 0% |
| reactions | 15 | 0 | 0 | 0 | 15 | **0%** | 0% |
| billing | 13 | 0 | 0 | 0 | 13 | **0%** | 0% |
| git | 13 | 2 | 0 | 0 | 11 | **15.4%** | 100% |
| checks | 12 | 0 | 0 | 0 | 12 | **0%** | 0% |
| security-advisories | 10 | 0 | 0 | 0 | 10 | **0%** | 0% |
| oidc | 8 | 0 | 0 | 0 | 8 | **0%** | 0% |
| search | 7 | 0 | 0 | 0 | 7 | **0%** | 0% |
| classroom | 6 | 0 | 0 | 0 | 6 | **0%** | 0% |
| private-registries | 6 | 0 | 0 | 0 | 6 | **0%** | 0% |
| hosted-compute | 6 | 0 | 0 | 0 | 6 | **0%** | 0% |
| meta | 5 | 2 | 0 | 0 | 3 | **40%** | 100% |
| agent-tasks | 5 | 0 | 0 | 0 | 5 | **0%** | 0% |
| campaigns | 5 | 0 | 0 | 0 | 5 | **0%** | 0% |
| dependency-graph | 5 | 0 | 0 | 0 | 5 | **0%** | 0% |
| code-quality | 4 | 0 | 0 | 0 | 4 | **0%** | 0% |
| licenses | 3 | 0 | 0 | 0 | 3 | **0%** | 0% |
| codes-of-conduct | 2 | 0 | 0 | 0 | 2 | **0%** | 0% |
| gitignore | 2 | 0 | 0 | 0 | 2 | **0%** | 0% |
| markdown | 2 | 0 | 0 | 0 | 2 | **0%** | 0% |
| credentials | 1 | 0 | 0 | 0 | 1 | **0%** | 0% |
| emojis | 1 | 0 | 0 | 0 | 1 | **0%** | 0% |
| rate-limit | 1 | 1 | 0 | 0 | 0 | **100%** | 100% |

## Failing operations (0)

_none_

## Conforming operations (24)

`meta/root`, `meta/get`, `rate-limit/get`, `repos/get`, `repos/list-branches`, `repos/get-branch`, `repos/list-commits`, `repos/get-commit`, `repos/get-content`, `git/get-commit`, `git/get-ref`, `issues/list-for-repo`, `issues/get`, `issues/list-comments`, `issues/list-labels-for-repo`, `issues/list-milestones`, `pulls/list`, `pulls/get`, `pulls/list-files`, `repos/list-releases`, `repos/get-latest-release`, `repos/get-release-by-tag`, `repos/list-tags`, `users/get-by-username`

## Untested tier 1/2 operations (209)

- `emojis/get` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `gists/list` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `gists/create` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `gists/get` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `gists/update` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `gists/delete` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `gitignore/get-all-templates` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `issues/list` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `licenses/get-all-commonly-used` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `markdown/render` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `activity/list-notifications-for-authenticated-user` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `meta/get-octocat` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `orgs/list` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `orgs/get` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `orgs/list-members` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `orgs/check-membership-for-user` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `repos/list-for-org` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `repos/create-in-org` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `teams/list` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `repos/update` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `repos/delete` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `actions/list-workflow-runs-for-repo` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `actions/get-workflow-run` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `actions/list-jobs-for-workflow-run` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `actions/list-repo-workflows` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `issues/list-assignees` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `issues/check-user-can-be-assigned` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `repos/get-branch-protection` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `repos/update-branch-protection` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `repos/delete-branch-protection` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `repos/rename-branch` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `checks/get` (tier 2): no scenario
- `checks/list-annotations` (tier 2): no scenario
- `checks/get-suite` (tier 2): no scenario
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
- `repos/list-branches-for-head-commit` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `repos/list-comments-for-commit` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `repos/create-commit-comment` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `repos/list-pull-requests-associated-with-commit` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `checks/list-for-ref` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `checks/list-suites-for-ref` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `repos/get-combined-status-for-ref` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `repos/list-commit-statuses-for-ref` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `repos/compare-commits` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
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
- `git/list-matching-refs` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
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
- `issues/update` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `issues/add-assignees` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `issues/remove-assignees` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
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
- `issues/create-label` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `issues/get-label` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `issues/update-label` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `issues/delete-label` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `repos/list-languages` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `licenses/get-for-repo` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `repos/merge-upstream` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `repos/merge` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `issues/create-milestone` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `issues/get-milestone` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `issues/update-milestone` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `issues/delete-milestone` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `pulls/create` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `pulls/list-review-comments-for-repo` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `pulls/get-review-comment` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `pulls/update-review-comment` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `pulls/delete-review-comment` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `reactions/list-for-pull-request-review-comment` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `reactions/create-for-pull-request-review-comment` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `reactions/delete-for-pull-request-comment` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `pulls/update` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `pulls/list-review-comments` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `pulls/create-review-comment` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `pulls/create-reply-for-review-comment` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `pulls/list-commits` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
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
- `repos/get-readme` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `repos/create-release` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `repos/get-release-asset` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `repos/update-release-asset` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `repos/delete-release-asset` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `repos/generate-release-notes` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
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
- `users/get-authenticated` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `users/update-authenticated` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `users/list-emails-for-authenticated-user` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `users/list-gpg-keys-for-authenticated-user` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `users/create-gpg-key-for-authenticated-user` (tier 1): no scenario
- `users/delete-gpg-key-for-authenticated-user` (tier 1): no scenario
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
- `users/list-followers-for-user` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `users/list-following-for-user` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `users/list-public-keys-for-user` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `orgs/list-for-user` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `repos/list-for-user` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `activity/list-repos-starred-by-user` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `meta/get-zen` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)

## Scenarios

- completed: ro: meta
- completed: ro: repository
- completed: ro: issues
- completed: ro: pulls
- completed: ro: releases

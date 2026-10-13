# REST conformance report

- **Target:** `http://localhost:3310/api/v3`
- **Spec:** `descriptions/ghes-3.22/ghes-3.22.json` @ [`58b1e0c00b`](https://github.com/github/rest-api-description/tree/58b1e0c00b39b9e46c24dc4a0ba6a2669c90e9c1) (API 1.1.4)
- **Run:** 2026-10-13T00:41:54.953Z · 269 HTTP requests · max tier 3

## Score

| scope | ops | conforming | nonconforming | not impl. | untested | % conforming | % of tested |
|---|---:|---:|---:|---:|---:|---:|---:|
| **all operations** | 1105 | 224 | 0 | 2 | 879 | **20.3%** | 99.1% |
| tier 1 (gh CLI core) | 104 | 102 | 0 | 0 | 2 | **98.1%** | 100% |
| tier 2 (octokit common) | 129 | 120 | 0 | 2 | 7 | **93%** | 98.4% |
| tier 3 (rest) | 872 | 2 | 0 | 0 | 870 | **0.2%** | 100% |

## Per tag

| tag | ops | conforming | nonconforming | not impl. | untested | % conforming | % of tested |
|---|---:|---:|---:|---:|---:|---:|---:|
| repos | 193 | 75 | 0 | 0 | 118 | **38.9%** | 100% |
| enterprise-admin | 171 | 0 | 0 | 0 | 171 | **0%** | – |
| actions | 160 | 2 | 0 | 0 | 158 | **1.3%** | 100% |
| orgs | 89 | 7 | 0 | 0 | 82 | **7.9%** | 100% |
| issues | 47 | 38 | 0 | 0 | 9 | **80.9%** | 100% |
| apps | 39 | 0 | 0 | 0 | 39 | **0%** | – |
| users | 37 | 13 | 0 | 0 | 24 | **35.1%** | 100% |
| teams | 35 | 1 | 0 | 0 | 34 | **2.9%** | 100% |
| activity | 32 | 13 | 0 | 0 | 19 | **40.6%** | 100% |
| dependabot | 28 | 0 | 0 | 0 | 28 | **0%** | – |
| packages | 27 | 0 | 0 | 0 | 27 | **0%** | – |
| pulls | 27 | 26 | 0 | 0 | 1 | **96.3%** | 100% |
| projects | 26 | 0 | 0 | 0 | 26 | **0%** | – |
| secret-scanning | 23 | 0 | 0 | 0 | 23 | **0%** | – |
| code-security | 20 | 0 | 0 | 0 | 20 | **0%** | – |
| gists | 20 | 5 | 0 | 0 | 15 | **25%** | 100% |
| code-scanning | 17 | 0 | 0 | 0 | 17 | **0%** | – |
| reactions | 15 | 12 | 0 | 2 | 1 | **80%** | 85.7% |
| git | 13 | 13 | 0 | 0 | 0 | **100%** | 100% |
| checks | 12 | 2 | 0 | 0 | 10 | **16.7%** | 100% |
| migrations | 11 | 0 | 0 | 0 | 11 | **0%** | – |
| oauth-authorizations | 10 | 0 | 0 | 0 | 10 | **0%** | – |
| search | 7 | 7 | 0 | 0 | 0 | **100%** | 100% |
| enterprise-team-organizations | 6 | 0 | 0 | 0 | 6 | **0%** | – |
| private-registries | 6 | 0 | 0 | 0 | 6 | **0%** | – |
| enterprise-teams | 5 | 0 | 0 | 0 | 5 | **0%** | – |
| enterprise-team-memberships | 5 | 0 | 0 | 0 | 5 | **0%** | – |
| meta | 4 | 4 | 0 | 0 | 0 | **100%** | 100% |
| licenses | 3 | 2 | 0 | 0 | 1 | **66.7%** | 100% |
| dependency-graph | 3 | 0 | 0 | 0 | 3 | **0%** | – |
| security-advisories | 2 | 0 | 0 | 0 | 2 | **0%** | – |
| codes-of-conduct | 2 | 0 | 0 | 0 | 2 | **0%** | – |
| billing | 2 | 0 | 0 | 0 | 2 | **0%** | – |
| gitignore | 2 | 1 | 0 | 0 | 1 | **50%** | 100% |
| markdown | 2 | 1 | 0 | 0 | 1 | **50%** | 100% |
| oidc | 2 | 0 | 0 | 0 | 2 | **0%** | – |
| emojis | 1 | 1 | 0 | 0 | 0 | **100%** | 100% |
| rate-limit | 1 | 1 | 0 | 0 | 0 | **100%** | 100% |

## Failing operations (2)

- **reactions/list-for-pull-request-review-comment** (tier 2, `GET /repos/{owner}/{repo}/pulls/comments/{comment_id}/reactions`): not_implemented
  - `404 for a route that should exist (expected 200)`
- **reactions/create-for-pull-request-review-comment** (tier 2, `POST /repos/{owner}/{repo}/pulls/comments/{comment_id}/reactions`): not_implemented
  - `404 for a route that should exist (expected 200/201)`

## Conforming operations (224)

`meta/root`, `emojis/get`, `gists/list`, `gists/create`, `gists/get`, `gists/update`, `gists/delete`, `gitignore/get-all-templates`, `issues/list`, `licenses/get-all-commonly-used`, `markdown/render`, `meta/get`, `activity/list-notifications-for-authenticated-user`, `meta/get-octocat`, `orgs/list`, `orgs/get`, `orgs/list-members`, `orgs/check-membership-for-user`, `repos/list-for-org`, `repos/create-in-org`, `teams/list`, `rate-limit/get`, `repos/get`, `repos/update`, `repos/delete`, `actions/list-workflow-runs-for-repo`, `actions/list-repo-workflows`, `issues/list-assignees`, `issues/check-user-can-be-assigned`, `repos/list-branches`, `repos/get-branch`, `repos/get-branch-protection`, `repos/update-branch-protection`, `repos/delete-branch-protection`, `repos/rename-branch`, `repos/list-collaborators`, `repos/check-collaborator`, `repos/add-collaborator`, `repos/remove-collaborator`, `repos/get-collaborator-permission-level`, `repos/list-commit-comments-for-repo`, `repos/get-commit-comment`, `repos/update-commit-comment`, `repos/delete-commit-comment`, `reactions/list-for-commit-comment`, `reactions/create-for-commit-comment`, `reactions/delete-for-commit-comment`, `repos/list-commits`, `repos/list-branches-for-head-commit`, `repos/list-comments-for-commit`, `repos/create-commit-comment`, `repos/list-pull-requests-associated-with-commit`, `repos/get-commit`, `checks/list-for-ref`, `checks/list-suites-for-ref`, `repos/get-combined-status-for-ref`, `repos/list-commit-statuses-for-ref`, `repos/compare-commits`, `repos/get-content`, `repos/create-or-update-file-contents`, `repos/delete-file`, `repos/list-contributors`, `repos/list-deployments`, `repos/create-deployment`, `repos/get-deployment`, `repos/delete-deployment`, `repos/list-deployment-statuses`, `repos/create-deployment-status`, `activity/list-repo-events`, `repos/list-forks`, `repos/create-fork`, `git/create-blob`, `git/get-blob`, `git/create-commit`, `git/get-commit`, `git/list-matching-refs`, `git/get-ref`, `git/create-ref`, `git/update-ref`, `git/delete-ref`, `git/create-tag`, `git/get-tag`, `git/create-tree`, `git/get-tree`, `repos/list-webhooks`, `repos/create-webhook`, `repos/get-webhook`, `repos/update-webhook`, `repos/delete-webhook`, `repos/ping-webhook`, `repos/list-invitations`, `issues/list-for-repo`, `issues/create`, `issues/list-comments-for-repo`, `issues/get-comment`, `issues/update-comment`, `issues/delete-comment`, `reactions/list-for-issue-comment`, `reactions/create-for-issue-comment`, `reactions/delete-for-issue-comment`, `issues/list-events-for-repo`, `issues/get-event`, `issues/get`, `issues/update`, `issues/add-assignees`, `issues/remove-assignees`, `issues/list-comments`, `issues/create-comment`, `issues/list-events`, `issues/list-labels-on-issue`, `issues/set-labels`, `issues/add-labels`, `issues/remove-all-labels`, `issues/remove-label`, `issues/lock`, `issues/unlock`, `reactions/list-for-issue`, `reactions/create-for-issue`, `reactions/delete-for-issue`, `issues/list-events-for-timeline`, `issues/list-labels-for-repo`, `issues/create-label`, `issues/get-label`, `issues/update-label`, `issues/delete-label`, `repos/list-languages`, `licenses/get-for-repo`, `repos/merge-upstream`, `repos/merge`, `issues/list-milestones`, `issues/create-milestone`, `issues/get-milestone`, `issues/update-milestone`, `issues/delete-milestone`, `issues/list-labels-for-milestone`, `pulls/list`, `pulls/create`, `pulls/list-review-comments-for-repo`, `pulls/get-review-comment`, `pulls/update-review-comment`, `pulls/delete-review-comment`, `pulls/get`, `pulls/update`, `pulls/list-review-comments`, `pulls/create-review-comment`, `pulls/create-reply-for-review-comment`, `pulls/list-commits`, `pulls/list-files`, `pulls/check-if-merged`, `pulls/merge`, `pulls/list-requested-reviewers`, `pulls/request-reviewers`, `pulls/remove-requested-reviewers`, `pulls/list-reviews`, `pulls/create-review`, `pulls/get-review`, `pulls/update-review`, `pulls/delete-pending-review`, `pulls/list-comments-for-review`, `pulls/submit-review`, `pulls/update-branch`, `repos/get-readme`, `repos/list-releases`, `repos/create-release`, `repos/get-release-asset`, `repos/update-release-asset`, `repos/delete-release-asset`, `repos/generate-release-notes`, `repos/get-latest-release`, `repos/get-release-by-tag`, `repos/get-release`, `repos/update-release`, `repos/delete-release`, `repos/list-release-assets`, `repos/upload-release-asset`, `reactions/list-for-release`, `reactions/create-for-release`, `reactions/delete-for-release`, `activity/list-stargazers-for-repo`, `repos/create-commit-status`, `activity/list-watchers-for-repo`, `activity/get-repo-subscription`, `activity/set-repo-subscription`, `activity/delete-repo-subscription`, `repos/list-tags`, `repos/download-tarball-archive`, `repos/get-all-topics`, `repos/replace-all-topics`, `repos/download-zipball-archive`, `repos/create-using-template`, `search/code`, `search/commits`, `search/issues-and-pull-requests`, `search/labels`, `search/repos`, `search/topics`, `search/users`, `users/get-authenticated`, `users/update-authenticated`, `users/list-emails-for-authenticated-user`, `users/list-gpg-keys-for-authenticated-user`, `issues/list-for-authenticated-user`, `users/list-public-ssh-keys-for-authenticated-user`, `users/create-public-ssh-key-for-authenticated-user`, `users/get-public-ssh-key-for-authenticated-user`, `users/delete-public-ssh-key-for-authenticated-user`, `orgs/get-membership-for-authenticated-user`, `orgs/list-for-authenticated-user`, `repos/list-for-authenticated-user`, `repos/create-for-authenticated-user`, `activity/list-repos-starred-by-authenticated-user`, `activity/check-repo-is-starred-by-authenticated-user`, `activity/star-repo-for-authenticated-user`, `activity/unstar-repo-for-authenticated-user`, `activity/list-watched-repos-for-authenticated-user`, `users/list`, `users/get-by-username`, `users/list-followers-for-user`, `users/list-following-for-user`, `users/list-public-keys-for-user`, `orgs/list-for-user`, `repos/list-for-user`, `activity/list-repos-starred-by-user`, `meta/get-zen`

## Untested tier 1/2 operations (9)

- `actions/get-workflow-run` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `actions/list-jobs-for-workflow-run` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `checks/get` (tier 2): no scenario
- `checks/list-annotations` (tier 2): no scenario
- `checks/get-suite` (tier 2): no scenario
- `reactions/delete-for-pull-request-comment` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `pulls/dismiss-review` (tier 2): no scenario
- `users/create-gpg-key-for-authenticated-user` (tier 1): no scenario
- `users/delete-gpg-key-for-authenticated-user` (tier 1): no scenario

## Scenarios

- completed: identity & meta
- completed: repository: create (fixture)
- completed: repository: metadata
- completed: contents API
- completed: git data & refs
- completed: branches, merges, protection, archives
- completed: commits, compare, statuses, checks, commit comments
- completed: labels & milestones
- completed: issues
- completed: issue comments & reactions
- completed: pull requests: create, review comments, reviews
- completed: pull requests: update-branch & close
- completed: pull requests: merge
- completed: releases & assets
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

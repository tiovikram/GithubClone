# REST conformance report

- **Target:** `http://localhost:3320/api/v1`
- **Spec:** `descriptions/ghes-3.22/ghes-3.22.json` @ [`58b1e0c00b`](https://github.com/github/rest-api-description/tree/58b1e0c00b39b9e46c24dc4a0ba6a2669c90e9c1) (API 1.1.4)
- **Run:** 2026-10-13T00:41:56.738Z · 174 HTTP requests · max tier 3

## Score

| scope | ops | conforming | nonconforming | not impl. | untested | % conforming | % of tested |
|---|---:|---:|---:|---:|---:|---:|---:|
| **all operations** | 1105 | 29 | 66 | 50 | 960 | **2.6%** | 20% |
| tier 1 (gh CLI core) | 104 | 10 | 49 | 25 | 20 | **9.6%** | 11.9% |
| tier 2 (octokit common) | 129 | 19 | 16 | 24 | 70 | **14.7%** | 32.2% |
| tier 3 (rest) | 872 | 0 | 1 | 1 | 870 | **0%** | 0% |

## Per tag

| tag | ops | conforming | nonconforming | not impl. | untested | % conforming | % of tested |
|---|---:|---:|---:|---:|---:|---:|---:|
| repos | 193 | 7 | 25 | 11 | 150 | **3.6%** | 16.3% |
| enterprise-admin | 171 | 0 | 0 | 0 | 171 | **0%** | – |
| actions | 160 | 1 | 0 | 1 | 158 | **0.6%** | 50% |
| orgs | 89 | 1 | 4 | 2 | 82 | **1.1%** | 14.3% |
| issues | 47 | 5 | 20 | 11 | 11 | **10.6%** | 13.9% |
| apps | 39 | 0 | 0 | 0 | 39 | **0%** | – |
| users | 37 | 6 | 5 | 2 | 24 | **16.2%** | 46.2% |
| teams | 35 | 0 | 1 | 0 | 34 | **0%** | 0% |
| activity | 32 | 7 | 5 | 1 | 19 | **21.9%** | 53.8% |
| dependabot | 28 | 0 | 0 | 0 | 28 | **0%** | – |
| packages | 27 | 0 | 0 | 0 | 27 | **0%** | – |
| pulls | 27 | 0 | 0 | 0 | 27 | **0%** | – |
| projects | 26 | 0 | 0 | 0 | 26 | **0%** | – |
| secret-scanning | 23 | 0 | 0 | 0 | 23 | **0%** | – |
| code-security | 20 | 0 | 0 | 0 | 20 | **0%** | – |
| gists | 20 | 0 | 0 | 2 | 18 | **0%** | 0% |
| code-scanning | 17 | 0 | 0 | 0 | 17 | **0%** | – |
| reactions | 15 | 0 | 4 | 2 | 9 | **0%** | 0% |
| git | 13 | 0 | 1 | 4 | 8 | **0%** | 0% |
| checks | 12 | 0 | 0 | 0 | 12 | **0%** | – |
| migrations | 11 | 0 | 0 | 0 | 11 | **0%** | – |
| oauth-authorizations | 10 | 0 | 0 | 0 | 10 | **0%** | – |
| search | 7 | 0 | 0 | 7 | 0 | **0%** | 0% |
| enterprise-team-organizations | 6 | 0 | 0 | 0 | 6 | **0%** | – |
| private-registries | 6 | 0 | 0 | 0 | 6 | **0%** | – |
| enterprise-teams | 5 | 0 | 0 | 0 | 5 | **0%** | – |
| enterprise-team-memberships | 5 | 0 | 0 | 0 | 5 | **0%** | – |
| meta | 4 | 0 | 0 | 4 | 0 | **0%** | 0% |
| licenses | 3 | 0 | 1 | 1 | 1 | **0%** | 0% |
| dependency-graph | 3 | 0 | 0 | 0 | 3 | **0%** | – |
| security-advisories | 2 | 0 | 0 | 0 | 2 | **0%** | – |
| codes-of-conduct | 2 | 0 | 0 | 0 | 2 | **0%** | – |
| billing | 2 | 0 | 0 | 0 | 2 | **0%** | – |
| gitignore | 2 | 1 | 0 | 0 | 1 | **50%** | 100% |
| markdown | 2 | 1 | 0 | 0 | 1 | **50%** | 100% |
| oidc | 2 | 0 | 0 | 0 | 2 | **0%** | – |
| emojis | 1 | 0 | 0 | 1 | 0 | **0%** | 0% |
| rate-limit | 1 | 0 | 0 | 1 | 0 | **0%** | 0% |

## Failing operations (116)

- **meta/root** (tier 1, `GET /`): not_implemented
  - `404 for a route that should exist (expected 200)`
- **issues/list** (tier 1, `GET /issues`): not_implemented
  - `404 for a route that should exist (expected 200)`
- **meta/get** (tier 1, `GET /meta`): not_implemented
  - `404 for a route that should exist (expected 200)`
- **orgs/get** (tier 1, `GET /orgs/{org}`): nonconforming
  - `schema: (root) missing required properties: login, url, node_id, repos_url, events_url, hooks_url, issues_url, members_url, public_members_url, html_url, has_organization_projects, has_repository_projects, ... (+8)  [gh-spec#/definitions/organization-full/required]`
  - `schema: /email must match format "email"  [gh-spec#/definitions/organization-full/properties/email/format]`
- **orgs/list-members** (tier 1, `GET /orgs/{org}/members`): nonconforming
  - `schema: /0 missing required properties: events_url, followers_url, following_url, gists_url, gravatar_id, node_id, organizations_url, received_events_url, repos_url, site_admin, starred_url, subscriptions_url, ... (+2)  [gh-spec#/definitions/simple-user/required]`
- **repos/list-for-org** (tier 1, `GET /orgs/{org}/repos`): nonconforming
  - `schema: /0 missing required properties: archive_url, assignees_url, blobs_url, branches_url, collaborators_url, comments_url, commits_url, compare_url, contents_url, contributors_url, deployments_url, downloads_url, ... (+24)  [#/required]`
  - `schema: /0/owner missing required properties: events_url, followers_url, following_url, gists_url, gravatar_id, node_id, organizations_url, received_events_url, repos_url, site_admin, starred_url, subscriptions_url, ... (+2)  [gh-spec#/definitions/simple-user/required]`
- **repos/create-in-org** (tier 1, `POST /orgs/{org}/repos`): nonconforming
  - `schema: (root) missing required properties: archive_url, assignees_url, blobs_url, branches_url, collaborators_url, comments_url, commits_url, compare_url, contents_url, contributors_url, deployments_url, downloads_url, ... (+39)  [#/required]`
  - `schema: /owner missing required properties: events_url, followers_url, following_url, gists_url, gravatar_id, node_id, organizations_url, received_events_url, repos_url, site_admin, starred_url, subscriptions_url, ... (+2)  [gh-spec#/definitions/simple-user/required]`
  - `schema: /parent must be object  [#/type]`
- **rate-limit/get** (tier 1, `GET /rate_limit`): not_implemented
  - `404 for a route that should exist (expected 200)`
- **repos/get** (tier 1, `GET /repos/{owner}/{repo}`): nonconforming
  - `schema: (root) missing required properties: archive_url, assignees_url, blobs_url, branches_url, collaborators_url, comments_url, commits_url, compare_url, contents_url, contributors_url, deployments_url, downloads_url, ... (+39)  [#/required]`
  - `schema: /owner missing required properties: events_url, followers_url, following_url, gists_url, gravatar_id, node_id, organizations_url, received_events_url, repos_url, site_admin, starred_url, subscriptions_url, ... (+2)  [gh-spec#/definitions/simple-user/required]`
  - `schema: /parent must be object  [#/type]`
- **repos/update** (tier 1, `PATCH /repos/{owner}/{repo}`): nonconforming
  - `schema: (root) missing required properties: archive_url, assignees_url, blobs_url, branches_url, collaborators_url, comments_url, commits_url, compare_url, contents_url, contributors_url, deployments_url, downloads_url, ... (+39)  [#/required]`
  - `schema: /owner missing required properties: events_url, followers_url, following_url, gists_url, gravatar_id, node_id, organizations_url, received_events_url, repos_url, site_admin, starred_url, subscriptions_url, ... (+2)  [gh-spec#/definitions/simple-user/required]`
  - `schema: /parent must be object  [#/type]`
- **issues/list-assignees** (tier 1, `GET /repos/{owner}/{repo}/assignees`): nonconforming
  - `schema: /0 missing required properties: events_url, followers_url, following_url, gists_url, gravatar_id, node_id, organizations_url, received_events_url, repos_url, site_admin, starred_url, subscriptions_url, ... (+2)  [gh-spec#/definitions/simple-user/required]`
- **repos/get-content** (tier 1, `GET /repos/{owner}/{repo}/contents/{path}`): nonconforming
  - `schema: (root) must be array  [gh-spec#/definitions/content-directory/type]`
  - `schema: /target must be string  [gh-spec#/definitions/content-file/properties/target/type]`
  - `schema: /submodule_git_url must be string  [gh-spec#/definitions/content-file/properties/submodule_git_url/type]`
  - `schema: /type must be equal to one of the allowed values ["symlink"]  [gh-spec#/definitions/content-symlink/properties/type/enum]`
- **repos/create-or-update-file-contents** (tier 1, `PUT /repos/{owner}/{repo}/contents/{path}`): nonconforming
  - `unexpected status 422 (scenario expected 201): {"message":"[SHA]: Required","url":"http://localhost:3320/api/swagger"} `
  - `schema: (root) missing required property: documentation_url  [gh-spec#/definitions/validation-error/required]`
- **repos/list-forks** (tier 1, `GET /repos/{owner}/{repo}/forks`): nonconforming
  - `schema: /0 missing required properties: archive_url, assignees_url, blobs_url, branches_url, collaborators_url, comments_url, commits_url, compare_url, contents_url, contributors_url, deployments_url, downloads_url, ... (+24)  [#/required]`
  - `schema: /0/owner missing required properties: events_url, followers_url, following_url, gists_url, gravatar_id, node_id, organizations_url, received_events_url, repos_url, site_admin, starred_url, subscriptions_url, ... (+2)  [gh-spec#/definitions/simple-user/required]`
- **repos/create-fork** (tier 1, `POST /repos/{owner}/{repo}/forks`): nonconforming
  - `schema: (root) missing required properties: archive_url, assignees_url, blobs_url, branches_url, collaborators_url, comments_url, commits_url, compare_url, contents_url, contributors_url, deployments_url, downloads_url, ... (+39)  [#/required]`
  - `schema: /owner missing required properties: events_url, followers_url, following_url, gists_url, gravatar_id, node_id, organizations_url, received_events_url, repos_url, site_admin, starred_url, subscriptions_url, ... (+2)  [gh-spec#/definitions/simple-user/required]`
  - `schema: /parent missing required properties: archive_url, assignees_url, blobs_url, branches_url, collaborators_url, comments_url, commits_url, compare_url, contents_url, contributors_url, deployments_url, downloads_url, ... (+37)  [#/required]`
  - `schema: /parent/owner missing required properties: events_url, followers_url, following_url, gists_url, gravatar_id, node_id, organizations_url, received_events_url, repos_url, site_admin, starred_url, subscriptions_url, ... (+2)  [gh-spec#/definitions/simple-user/required]`
- **git/list-matching-refs** (tier 1, `GET /repos/{owner}/{repo}/git/matching-refs/{ref}`): not_implemented
  - `404 for a route that should exist (expected 200)`
- **git/get-ref** (tier 1, `GET /repos/{owner}/{repo}/git/ref/{ref}`): not_implemented
  - `404 for a route that should exist (expected 200)`
- **git/create-ref** (tier 1, `POST /repos/{owner}/{repo}/git/refs`): not_implemented
  - `405 for a route that should exist (expected 201)`
- **issues/list-for-repo** (tier 1, `GET /repos/{owner}/{repo}/issues`): nonconforming
  - `schema: /0 missing required properties: comments_url, events_url, node_id, labels_url, repository_url, locked  [#/required]`
  - `schema: /0/user missing required properties: events_url, followers_url, following_url, gists_url, gravatar_id, node_id, organizations_url, received_events_url, repos_url, site_admin, starred_url, subscriptions_url, ... (+2)  [gh-spec#/definitions/nullable-simple-user/required]`
  - `schema: /0/assignees must be array  [#/properties/assignees/type]`
  - `schema: /0/pull_request must be object  [#/properties/pull_request/type]`
- **issues/create** (tier 1, `POST /repos/{owner}/{repo}/issues`): nonconforming
  - `unexpected status 422 (scenario expected 201): {"message":"[]: json: cannot unmarshal string into Go struct field CreateIssueOption.labels of type int64","url":"http://localhost:3320/api/swagger"} `
  - `schema: (root) missing required property: documentation_url  [gh-spec#/definitions/validation-error/required]`
  - `schema: (root) missing required properties: comments_url, events_url, node_id, labels_url, repository_url, locked  [#/required]`
  - `schema: /user missing required properties: events_url, followers_url, following_url, gists_url, gravatar_id, node_id, organizations_url, received_events_url, repos_url, site_admin, starred_url, subscriptions_url, ... (+2)  [gh-spec#/definitions/nullable-simple-user/required]`
- **issues/list-comments-for-repo** (tier 1, `GET /repos/{owner}/{repo}/issues/comments`): nonconforming
  - `schema: /0 missing required properties: node_id, url  [#/required]`
  - `schema: /0/user missing required properties: events_url, followers_url, following_url, gists_url, gravatar_id, node_id, organizations_url, received_events_url, repos_url, site_admin, starred_url, subscriptions_url, ... (+2)  [gh-spec#/definitions/nullable-simple-user/required]`
  - `schema: /1 missing required properties: node_id, url  [#/required]`
  - `schema: /1/user missing required properties: events_url, followers_url, following_url, gists_url, gravatar_id, node_id, organizations_url, received_events_url, repos_url, site_admin, starred_url, subscriptions_url, ... (+2)  [gh-spec#/definitions/nullable-simple-user/required]`
- **issues/get-comment** (tier 1, `GET /repos/{owner}/{repo}/issues/comments/{comment_id}`): nonconforming
  - `schema: (root) missing required properties: node_id, url  [#/required]`
  - `schema: /user missing required properties: events_url, followers_url, following_url, gists_url, gravatar_id, node_id, organizations_url, received_events_url, repos_url, site_admin, starred_url, subscriptions_url, ... (+2)  [gh-spec#/definitions/nullable-simple-user/required]`
- **issues/update-comment** (tier 1, `PATCH /repos/{owner}/{repo}/issues/comments/{comment_id}`): nonconforming
  - `schema: (root) missing required properties: node_id, url  [#/required]`
  - `schema: /user missing required properties: events_url, followers_url, following_url, gists_url, gravatar_id, node_id, organizations_url, received_events_url, repos_url, site_admin, starred_url, subscriptions_url, ... (+2)  [gh-spec#/definitions/nullable-simple-user/required]`
- **issues/get** (tier 1, `GET /repos/{owner}/{repo}/issues/{issue_number}`): nonconforming
  - `schema: (root) missing required properties: comments_url, events_url, node_id, labels_url, repository_url, locked  [#/required]`
  - `schema: /user missing required properties: events_url, followers_url, following_url, gists_url, gravatar_id, node_id, organizations_url, received_events_url, repos_url, site_admin, starred_url, subscriptions_url, ... (+2)  [gh-spec#/definitions/nullable-simple-user/required]`
  - `schema: /assignees must be array  [#/properties/assignees/type]`
  - `schema: /pull_request must be object  [#/properties/pull_request/type]`
- **issues/update** (tier 1, `PATCH /repos/{owner}/{repo}/issues/{issue_number}`): nonconforming
  - `status 201 not documented (documented: 200, 301, 403, 404, 410, 422, 503)`
  - `unexpected status 201 (scenario expected 200): {"id":1,"url":"http://localhost:3320/api/v1/repos/conf/conf-mv5yhhxeoj4b/issues/1","html_url":"http://localhost:3320/conf/conf-mv5yhhxeoj4b/issues/1","number":1`
  - `unexpected status 201 (scenario expected 200): {"id":2,"url":"http://localhost:3320/api/v1/repos/conf/conf-mv5yhhxeoj4b/issues/2","html_url":"http://localhost:3320/conf/conf-mv5yhhxeoj4b/issues/2","number":2`
- **issues/add-assignees** (tier 1, `POST /repos/{owner}/{repo}/issues/{issue_number}/assignees`): not_implemented
  - `404 for a route that should exist (expected 201)`
- **issues/remove-assignees** (tier 1, `DELETE /repos/{owner}/{repo}/issues/{issue_number}/assignees`): not_implemented
  - `404 for a route that should exist (expected 200)`
- **issues/list-comments** (tier 1, `GET /repos/{owner}/{repo}/issues/{issue_number}/comments`): nonconforming
  - `schema: /0 missing required properties: node_id, url  [#/required]`
  - `schema: /0/user missing required properties: events_url, followers_url, following_url, gists_url, gravatar_id, node_id, organizations_url, received_events_url, repos_url, site_admin, starred_url, subscriptions_url, ... (+2)  [gh-spec#/definitions/nullable-simple-user/required]`
  - `schema: /1 missing required properties: node_id, url  [#/required]`
  - `schema: /1/user missing required properties: events_url, followers_url, following_url, gists_url, gravatar_id, node_id, organizations_url, received_events_url, repos_url, site_admin, starred_url, subscriptions_url, ... (+2)  [gh-spec#/definitions/nullable-simple-user/required]`
- **issues/create-comment** (tier 1, `POST /repos/{owner}/{repo}/issues/{issue_number}/comments`): nonconforming
  - `schema: (root) missing required properties: node_id, url  [#/required]`
  - `schema: /user missing required properties: events_url, followers_url, following_url, gists_url, gravatar_id, node_id, organizations_url, received_events_url, repos_url, site_admin, starred_url, subscriptions_url, ... (+2)  [gh-spec#/definitions/nullable-simple-user/required]`
- **issues/set-labels** (tier 1, `PUT /repos/{owner}/{repo}/issues/{issue_number}/labels`): nonconforming
  - `schema: /0 missing required properties: node_id, default, archived_at, archived_by  [#/required]`
  - `schema: /1 missing required properties: node_id, default, archived_at, archived_by  [#/required]`
- **issues/add-labels** (tier 1, `POST /repos/{owner}/{repo}/issues/{issue_number}/labels`): nonconforming
  - `schema: /0 missing required properties: node_id, default, archived_at, archived_by  [#/required]`
- **issues/remove-label** (tier 1, `DELETE /repos/{owner}/{repo}/issues/{issue_number}/labels/{name}`): nonconforming
  - `schema: /0 missing required properties: node_id, default, archived_at, archived_by  [#/required]`
- **issues/lock** (tier 1, `PUT /repos/{owner}/{repo}/issues/{issue_number}/lock`): not_implemented
  - `404 for a route that should exist (expected 204)`
- **issues/unlock** (tier 1, `DELETE /repos/{owner}/{repo}/issues/{issue_number}/lock`): not_implemented
  - `404 for a route that should exist (expected 204)`
- **issues/list-labels-for-repo** (tier 1, `GET /repos/{owner}/{repo}/labels`): nonconforming
  - `schema: /0 missing required properties: node_id, default, archived_at, archived_by  [#/required]`
  - `schema: /1 missing required properties: node_id, default, archived_at, archived_by  [#/required]`
  - `schema: /2 missing required properties: node_id, default, archived_at, archived_by  [#/required]`
  - `per_page=1 returned 3 items`
- **issues/create-label** (tier 1, `POST /repos/{owner}/{repo}/labels`): nonconforming
  - `schema: (root) missing required properties: node_id, default, archived_at, archived_by  [#/required]`
- **issues/get-label** (tier 1, `GET /repos/{owner}/{repo}/labels/{name}`): nonconforming
  - `schema: (root) missing required properties: node_id, default, archived_at, archived_by  [#/required]`
- **issues/update-label** (tier 1, `PATCH /repos/{owner}/{repo}/labels/{name}`): not_implemented
  - `404 for a route that should exist (expected 200)`
- **repos/merge-upstream** (tier 1, `POST /repos/{owner}/{repo}/merge-upstream`): not_implemented
  - `404 for a route that should exist (expected 200)`
- **issues/list-milestones** (tier 1, `GET /repos/{owner}/{repo}/milestones`): nonconforming
  - `schema: /0 missing required properties: creator, node_id, labels_url, html_url, number, url  [#/required]`
- **issues/create-milestone** (tier 1, `POST /repos/{owner}/{repo}/milestones`): nonconforming
  - `schema: (root) missing required properties: creator, node_id, labels_url, html_url, number, url  [#/required]`
- **issues/get-milestone** (tier 1, `GET /repos/{owner}/{repo}/milestones/{milestone_number}`): nonconforming
  - `schema: (root) missing required properties: creator, node_id, labels_url, html_url, number, url  [#/required]`
- **issues/update-milestone** (tier 1, `PATCH /repos/{owner}/{repo}/milestones/{milestone_number}`): nonconforming
  - `schema: (root) missing required properties: creator, node_id, labels_url, html_url, number, url  [#/required]`
- **repos/get-readme** (tier 1, `GET /repos/{owner}/{repo}/readme`): not_implemented
  - `404 for a route that should exist (expected 200)`
- **repos/list-releases** (tier 1, `GET /repos/{owner}/{repo}/releases`): nonconforming
  - `schema: /0 missing required properties: assets_url, node_id  [#/required]`
  - `schema: /0/author missing required properties: events_url, followers_url, following_url, gists_url, gravatar_id, node_id, organizations_url, received_events_url, repos_url, site_admin, starred_url, subscriptions_url, ... (+2)  [gh-spec#/definitions/simple-user/required]`
- **repos/create-release** (tier 1, `POST /repos/{owner}/{repo}/releases`): nonconforming
  - `schema: (root) missing required properties: assets_url, node_id  [#/required]`
  - `schema: /author missing required properties: events_url, followers_url, following_url, gists_url, gravatar_id, node_id, organizations_url, received_events_url, repos_url, site_admin, starred_url, subscriptions_url, ... (+2)  [gh-spec#/definitions/simple-user/required]`
- **repos/get-release-asset** (tier 1, `GET /repos/{owner}/{repo}/releases/assets/{asset_id}`): not_implemented
  - `404 for a route that should exist (expected 200)`
- **repos/update-release-asset** (tier 1, `PATCH /repos/{owner}/{repo}/releases/assets/{asset_id}`): not_implemented
  - `404 for a route that should exist (expected 200)`
- **repos/delete-release-asset** (tier 1, `DELETE /repos/{owner}/{repo}/releases/assets/{asset_id}`): not_implemented
  - `404 for a route that should exist (expected 204)`
- **repos/generate-release-notes** (tier 1, `POST /repos/{owner}/{repo}/releases/generate-notes`): not_implemented
  - `405 for a route that should exist (expected 200)`
- **repos/get-latest-release** (tier 1, `GET /repos/{owner}/{repo}/releases/latest`): nonconforming
  - `schema: (root) missing required properties: assets_url, node_id  [#/required]`
  - `schema: /author missing required properties: events_url, followers_url, following_url, gists_url, gravatar_id, node_id, organizations_url, received_events_url, repos_url, site_admin, starred_url, subscriptions_url, ... (+2)  [gh-spec#/definitions/simple-user/required]`
- **repos/get-release-by-tag** (tier 1, `GET /repos/{owner}/{repo}/releases/tags/{tag}`): nonconforming
  - `schema: (root) missing required properties: assets_url, node_id  [#/required]`
  - `schema: /author missing required properties: events_url, followers_url, following_url, gists_url, gravatar_id, node_id, organizations_url, received_events_url, repos_url, site_admin, starred_url, subscriptions_url, ... (+2)  [gh-spec#/definitions/simple-user/required]`
- **repos/get-release** (tier 1, `GET /repos/{owner}/{repo}/releases/{release_id}`): nonconforming
  - `schema: (root) missing required properties: assets_url, node_id  [#/required]`
  - `schema: /author missing required properties: events_url, followers_url, following_url, gists_url, gravatar_id, node_id, organizations_url, received_events_url, repos_url, site_admin, starred_url, subscriptions_url, ... (+2)  [gh-spec#/definitions/simple-user/required]`
- **repos/update-release** (tier 1, `PATCH /repos/{owner}/{repo}/releases/{release_id}`): nonconforming
  - `schema: (root) missing required properties: assets_url, node_id  [#/required]`
  - `schema: /author missing required properties: events_url, followers_url, following_url, gists_url, gravatar_id, node_id, organizations_url, received_events_url, repos_url, site_admin, starred_url, subscriptions_url, ... (+2)  [gh-spec#/definitions/simple-user/required]`
- **repos/list-release-assets** (tier 1, `GET /repos/{owner}/{repo}/releases/{release_id}/assets`): nonconforming
  - `schema: /0 missing required properties: content_type, digest, state, url, node_id, label, uploader, updated_at  [#/required]`
- **repos/upload-release-asset** (tier 1, `POST /repos/{owner}/{repo}/releases/{release_id}/assets`): nonconforming
  - `schema: (root) missing required properties: content_type, digest, state, url, node_id, label, uploader, updated_at  [#/required]`
- **repos/get-all-topics** (tier 1, `GET /repos/{owner}/{repo}/topics`): nonconforming
  - `schema: (root) missing required property: names  [gh-spec#/definitions/topic/required]`
- **repos/replace-all-topics** (tier 1, `PUT /repos/{owner}/{repo}/topics`): nonconforming
  - `status 204 not documented (documented: 200, 404, 422)`
  - `unexpected status 204 (scenario expected 200): `
- **repos/create-using-template** (tier 1, `POST /repos/{template_owner}/{template_repo}/generate`): nonconforming
  - `status 422 not documented (documented: 201)`
  - `unexpected status 422 (scenario expected 201): {"message":"this is not a template repo","url":"http://localhost:3320/api/swagger"} `
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
- **users/get-authenticated** (tier 1, `GET /user`): nonconforming
  - `schema: (root) missing required properties: events_url, followers_url, following_url, gists_url, gravatar_id, node_id, organizations_url, received_events_url, repos_url, site_admin, starred_url, subscriptions_url, ... (+19)  [gh-spec#/definitions/private-user/required]`
  - `schema: (root) missing required properties: events_url, followers_url, following_url, gists_url, gravatar_id, node_id, organizations_url, received_events_url, repos_url, site_admin, starred_url, subscriptions_url, ... (+13)  [gh-spec#/definitions/public-user/required]`
  - `schema: (root) must NOT have additional properties (login_name)  [gh-spec#/definitions/public-user/additionalProperties]`
- **issues/list-for-authenticated-user** (tier 1, `GET /user/issues`): not_implemented
  - `404 for a route that should exist (expected 200)`
- **users/create-public-ssh-key-for-authenticated-user** (tier 1, `POST /user/keys`): nonconforming
  - `schema: (root) missing required property: read_only  [gh-spec#/definitions/key/required]`
- **orgs/get-membership-for-authenticated-user** (tier 1, `GET /user/memberships/orgs/{org}`): not_implemented
  - `404 for a route that should exist (expected 200)`
- **orgs/list-for-authenticated-user** (tier 1, `GET /user/orgs`): nonconforming
  - `schema: /0 missing required properties: login, url, node_id, repos_url, events_url, hooks_url, issues_url, members_url, public_members_url  [gh-spec#/definitions/organization-simple/required]`
- **repos/list-for-authenticated-user** (tier 1, `GET /user/repos`): nonconforming
  - `schema: /0 missing required properties: archive_url, assignees_url, blobs_url, branches_url, collaborators_url, comments_url, commits_url, compare_url, contents_url, contributors_url, deployments_url, downloads_url, ... (+37)  [#/required]`
  - `schema: /0/owner missing required properties: events_url, followers_url, following_url, gists_url, gravatar_id, node_id, organizations_url, received_events_url, repos_url, site_admin, starred_url, subscriptions_url, ... (+2)  [gh-spec#/definitions/simple-user/required]`
- **repos/create-for-authenticated-user** (tier 1, `POST /user/repos`): nonconforming
  - `schema: (root) missing required properties: archive_url, assignees_url, blobs_url, branches_url, collaborators_url, comments_url, commits_url, compare_url, contents_url, contributors_url, deployments_url, downloads_url, ... (+39)  [#/required]`
  - `schema: /owner missing required properties: events_url, followers_url, following_url, gists_url, gravatar_id, node_id, organizations_url, received_events_url, repos_url, site_admin, starred_url, subscriptions_url, ... (+2)  [gh-spec#/definitions/simple-user/required]`
  - `schema: /parent must be object  [#/type]`
- **users/get-by-username** (tier 1, `GET /users/{username}`): nonconforming
  - `schema: (root) missing required properties: events_url, followers_url, following_url, gists_url, gravatar_id, node_id, organizations_url, received_events_url, repos_url, site_admin, starred_url, subscriptions_url, ... (+19)  [gh-spec#/definitions/private-user/required]`
  - `schema: (root) missing required properties: events_url, followers_url, following_url, gists_url, gravatar_id, node_id, organizations_url, received_events_url, repos_url, site_admin, starred_url, subscriptions_url, ... (+13)  [gh-spec#/definitions/public-user/required]`
  - `schema: (root) must NOT have additional properties (login_name)  [gh-spec#/definitions/public-user/additionalProperties]`
- **orgs/list-for-user** (tier 1, `GET /users/{username}/orgs`): nonconforming
  - `schema: /0 missing required properties: login, url, node_id, repos_url, events_url, hooks_url, issues_url, members_url, public_members_url  [gh-spec#/definitions/organization-simple/required]`
- **repos/list-for-user** (tier 1, `GET /users/{username}/repos`): nonconforming
  - `schema: /0 missing required properties: archive_url, assignees_url, blobs_url, branches_url, collaborators_url, comments_url, commits_url, compare_url, contents_url, contributors_url, deployments_url, downloads_url, ... (+24)  [#/required]`
  - `schema: /0/owner missing required properties: events_url, followers_url, following_url, gists_url, gravatar_id, node_id, organizations_url, received_events_url, repos_url, site_admin, starred_url, subscriptions_url, ... (+2)  [gh-spec#/definitions/simple-user/required]`
- **emojis/get** (tier 2, `GET /emojis`): not_implemented
  - `404 for a route that should exist (expected 200)`
- **gists/list** (tier 2, `GET /gists`): not_implemented
  - `404 for a route that should exist (expected 200)`
- **gists/create** (tier 2, `POST /gists`): not_implemented
  - `404 for a route that should exist (expected 201)`
- **licenses/get-all-commonly-used** (tier 2, `GET /licenses`): nonconforming
  - `schema: /0 missing required properties: spdx_id, node_id  [gh-spec#/definitions/license-simple/required]`
  - `schema: /1 missing required properties: spdx_id, node_id  [gh-spec#/definitions/license-simple/required]`
  - `schema: /2 missing required properties: spdx_id, node_id  [gh-spec#/definitions/license-simple/required]`
  - `schema: /3 missing required properties: spdx_id, node_id  [gh-spec#/definitions/license-simple/required]`
- **meta/get-octocat** (tier 2, `GET /octocat`): not_implemented
  - `404 for a route that should exist (expected 200)`
- **orgs/list** (tier 2, `GET /organizations`): not_implemented
  - `404 for a route that should exist (expected 200)`
- **teams/list** (tier 2, `GET /orgs/{org}/teams`): nonconforming
  - `schema: /0 missing required properties: node_id, url, members_url, html_url, repositories_url, slug, parent, type  [#/required]`
- **actions/list-repo-workflows** (tier 2, `GET /repos/{owner}/{repo}/actions/workflows`): not_implemented
  - `404 for a route that should exist (expected 200)`
- **issues/check-user-can-be-assigned** (tier 2, `GET /repos/{owner}/{repo}/assignees/{assignee}`): not_implemented
  - `404 for a route that should exist (expected 204)`
- **repos/list-collaborators** (tier 2, `GET /repos/{owner}/{repo}/collaborators`): nonconforming
  - `expected at least 1 item(s) (fixtures exist), got 0`
- **repos/check-collaborator** (tier 2, `GET /repos/{owner}/{repo}/collaborators/{username}`): not_implemented
  - `404 for a route that should exist (expected 204)`
- **repos/get-collaborator-permission-level** (tier 2, `GET /repos/{owner}/{repo}/collaborators/{username}/permission`): nonconforming
  - `schema: /user missing required properties: events_url, followers_url, following_url, gists_url, gravatar_id, node_id, organizations_url, received_events_url, repos_url, site_admin, starred_url, subscriptions_url, ... (+2)  [gh-spec#/definitions/nullable-collaborator/required]`
- **repos/list-contributors** (tier 2, `GET /repos/{owner}/{repo}/contributors`): not_implemented
  - `404 for a route that should exist (expected 200/204)`
- **repos/list-deployments** (tier 2, `GET /repos/{owner}/{repo}/deployments`): not_implemented
  - `404 for a route that should exist (expected 200)`
- **repos/create-deployment** (tier 2, `POST /repos/{owner}/{repo}/deployments`): not_implemented
  - `404 for a route that should exist (expected 201)`
- **activity/list-repo-events** (tier 2, `GET /repos/{owner}/{repo}/events`): not_implemented
  - `404 for a route that should exist (expected 200)`
- **git/create-blob** (tier 2, `POST /repos/{owner}/{repo}/git/blobs`): not_implemented
  - `405 for a route that should exist (expected 201)`
- **git/get-commit** (tier 2, `GET /repos/{owner}/{repo}/git/commits/{commit_sha}`): nonconforming
  - `schema: (root) missing required properties: node_id, tree, message, verification  [gh-spec#/definitions/git-commit/required]`
  - `schema: /author missing required properties: name, date  [gh-spec#/definitions/git-commit/properties/author/required]`
  - `schema: /committer missing required properties: name, date  [gh-spec#/definitions/git-commit/properties/committer/required]`
- **repos/create-webhook** (tier 2, `POST /repos/{owner}/{repo}/hooks`): nonconforming
  - `unexpected status 422 (scenario expected 201): {"message":"[Type]: Required","url":"http://localhost:3320/api/swagger"} `
  - `schema: (root) missing required property: documentation_url  [gh-spec#/definitions/validation-error/required]`
- **repos/list-invitations** (tier 2, `GET /repos/{owner}/{repo}/invitations`): not_implemented
  - `404 for a route that should exist (expected 200)`
- **reactions/list-for-issue-comment** (tier 2, `GET /repos/{owner}/{repo}/issues/comments/{comment_id}/reactions`): nonconforming
  - `schema: /0 missing required properties: id, node_id  [#/required]`
  - `schema: /0/user missing required properties: events_url, followers_url, following_url, gists_url, gravatar_id, node_id, organizations_url, received_events_url, repos_url, site_admin, starred_url, subscriptions_url, ... (+2)  [gh-spec#/definitions/nullable-simple-user/required]`
- **reactions/create-for-issue-comment** (tier 2, `POST /repos/{owner}/{repo}/issues/comments/{comment_id}/reactions`): nonconforming
  - `schema: (root) missing required properties: id, node_id  [#/required]`
  - `schema: /user missing required properties: events_url, followers_url, following_url, gists_url, gravatar_id, node_id, organizations_url, received_events_url, repos_url, site_admin, starred_url, subscriptions_url, ... (+2)  [gh-spec#/definitions/nullable-simple-user/required]`
- **issues/list-events-for-repo** (tier 2, `GET /repos/{owner}/{repo}/issues/events`): not_implemented
  - `404 for a route that should exist (expected 200)`
- **issues/list-events** (tier 2, `GET /repos/{owner}/{repo}/issues/{issue_number}/events`): not_implemented
  - `404 for a route that should exist (expected 200)`
- **reactions/list-for-issue** (tier 2, `GET /repos/{owner}/{repo}/issues/{issue_number}/reactions`): nonconforming
  - `schema: /0 missing required properties: id, node_id  [#/required]`
  - `schema: /0/user missing required properties: events_url, followers_url, following_url, gists_url, gravatar_id, node_id, organizations_url, received_events_url, repos_url, site_admin, starred_url, subscriptions_url, ... (+2)  [gh-spec#/definitions/nullable-simple-user/required]`
- **reactions/create-for-issue** (tier 2, `POST /repos/{owner}/{repo}/issues/{issue_number}/reactions`): nonconforming
  - `schema: (root) missing required properties: id, node_id  [#/required]`
  - `schema: /user missing required properties: events_url, followers_url, following_url, gists_url, gravatar_id, node_id, organizations_url, received_events_url, repos_url, site_admin, starred_url, subscriptions_url, ... (+2)  [gh-spec#/definitions/nullable-simple-user/required]`
- **licenses/get-for-repo** (tier 2, `GET /repos/{owner}/{repo}/license`): not_implemented
  - `404 for a route that should exist (expected 200)`
- **reactions/list-for-release** (tier 2, `GET /repos/{owner}/{repo}/releases/{release_id}/reactions`): not_implemented
  - `404 for a route that should exist (expected 200)`
- **reactions/create-for-release** (tier 2, `POST /repos/{owner}/{repo}/releases/{release_id}/reactions`): not_implemented
  - `404 for a route that should exist (expected 200/201)`
- **activity/list-stargazers-for-repo** (tier 2, `GET /repos/{owner}/{repo}/stargazers`): nonconforming
  - `schema: /0 missing required properties: events_url, followers_url, following_url, gists_url, gravatar_id, node_id, organizations_url, received_events_url, repos_url, site_admin, starred_url, subscriptions_url, ... (+2)  [gh-spec#/definitions/simple-user/required]`
  - `schema: /0 missing required properties: starred_at, user  [#/required]`
- **activity/list-watchers-for-repo** (tier 2, `GET /repos/{owner}/{repo}/subscribers`): nonconforming
  - `schema: /0 missing required properties: events_url, followers_url, following_url, gists_url, gravatar_id, node_id, organizations_url, received_events_url, repos_url, site_admin, starred_url, subscriptions_url, ... (+2)  [gh-spec#/definitions/simple-user/required]`
- **search/labels** (tier 2, `GET /search/labels`): not_implemented
  - `404 for a route that should exist (expected 200)`
- **search/topics** (tier 2, `GET /search/topics`): not_implemented
  - `404 for a route that should exist (expected 200)`
- **users/update-authenticated** (tier 2, `PATCH /user`): not_implemented
  - `405 for a route that should exist (expected 200)`
- **users/list-emails-for-authenticated-user** (tier 2, `GET /user/emails`): nonconforming
  - `schema: /0 missing required property: visibility  [gh-spec#/definitions/email/required]`
- **activity/list-repos-starred-by-authenticated-user** (tier 2, `GET /user/starred`): nonconforming
  - `schema: /0 missing required properties: archive_url, assignees_url, blobs_url, branches_url, collaborators_url, comments_url, commits_url, compare_url, contents_url, contributors_url, deployments_url, downloads_url, ... (+37)  [#/required]`
  - `schema: /0/owner missing required properties: events_url, followers_url, following_url, gists_url, gravatar_id, node_id, organizations_url, received_events_url, repos_url, site_admin, starred_url, subscriptions_url, ... (+2)  [gh-spec#/definitions/simple-user/required]`
- **activity/list-watched-repos-for-authenticated-user** (tier 2, `GET /user/subscriptions`): nonconforming
  - `schema: /0 missing required properties: archive_url, assignees_url, blobs_url, branches_url, collaborators_url, comments_url, commits_url, compare_url, contents_url, contributors_url, deployments_url, downloads_url, ... (+24)  [#/required]`
  - `schema: /0/owner missing required properties: events_url, followers_url, following_url, gists_url, gravatar_id, node_id, organizations_url, received_events_url, repos_url, site_admin, starred_url, subscriptions_url, ... (+2)  [gh-spec#/definitions/simple-user/required]`
- **users/list** (tier 2, `GET /users`): not_implemented
  - `404 for a route that should exist (expected 200)`
- **activity/list-repos-starred-by-user** (tier 2, `GET /users/{username}/starred`): nonconforming
  - `schema: /0 missing required properties: starred_at, repo, archive_url, assignees_url, blobs_url, branches_url, collaborators_url, comments_url, commits_url, compare_url, contents_url, contributors_url, ... (+39)  [#/required]`
  - `schema: /0/owner missing required properties: events_url, followers_url, following_url, gists_url, gravatar_id, node_id, organizations_url, received_events_url, repos_url, site_admin, starred_url, subscriptions_url, ... (+2)  [gh-spec#/definitions/simple-user/required]`
- **meta/get-zen** (tier 2, `GET /zen`): not_implemented
  - `404 for a route that should exist (expected 200)`
- **issues/list-labels-for-milestone** (tier 3, `GET /repos/{owner}/{repo}/milestones/{milestone_number}/labels`): not_implemented
  - `404 for a route that should exist (expected 200)`
- **users/get-public-ssh-key-for-authenticated-user** (tier 3, `GET /user/keys/{key_id}`): nonconforming
  - `schema: (root) missing required property: read_only  [gh-spec#/definitions/key/required]`

## Conforming operations (29)

`gitignore/get-all-templates`, `markdown/render`, `activity/list-notifications-for-authenticated-user`, `orgs/check-membership-for-user`, `repos/delete`, `actions/list-workflow-runs-for-repo`, `repos/add-collaborator`, `repos/remove-collaborator`, `repos/list-webhooks`, `issues/delete-comment`, `issues/list-labels-on-issue`, `issues/remove-all-labels`, `issues/list-events-for-timeline`, `issues/delete-label`, `repos/list-languages`, `repos/delete-release`, `activity/get-repo-subscription`, `activity/set-repo-subscription`, `activity/delete-repo-subscription`, `repos/list-tags`, `users/list-gpg-keys-for-authenticated-user`, `users/list-public-ssh-keys-for-authenticated-user`, `users/delete-public-ssh-key-for-authenticated-user`, `activity/check-repo-is-starred-by-authenticated-user`, `activity/star-repo-for-authenticated-user`, `activity/unstar-repo-for-authenticated-user`, `users/list-followers-for-user`, `users/list-following-for-user`, `users/list-public-keys-for-user`

## Untested tier 1/2 operations (90)

- `gists/get` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `gists/update` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `gists/delete` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `actions/get-workflow-run` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `actions/list-jobs-for-workflow-run` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `repos/list-branches` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `repos/get-branch` (tier 1): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `repos/get-branch-protection` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `repos/update-branch-protection` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `repos/delete-branch-protection` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `repos/rename-branch` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `checks/get` (tier 2): no scenario
- `checks/list-annotations` (tier 2): no scenario
- `checks/get-suite` (tier 2): no scenario
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
- `git/get-blob` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `git/create-commit` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
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
- `reactions/delete-for-issue-comment` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `issues/get-event` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `reactions/delete-for-issue` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `repos/merge` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
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
- `reactions/delete-for-release` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `repos/create-commit-status` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `repos/download-tarball-archive` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `repos/download-zipball-archive` (tier 2): scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)
- `users/create-gpg-key-for-authenticated-user` (tier 1): no scenario
- `users/delete-gpg-key-for-authenticated-user` (tier 1): no scenario

## Scenarios

- completed: identity & meta
- completed: repository: create (fixture)
- completed: repository: metadata
- blocked: contents API — fixture step repos/create-or-update-file-contents returned 422: {"message":"[SHA]: Required","url":"http://localhost:3320/api/swagger"}

- blocked: git data & refs — fixture step git/create-ref returned 405: 
- skipped: branches, merges, protection, archives — requires featureSha
- skipped: commits, compare, statuses, checks, commit comments — requires featureSha
- completed: labels & milestones
- completed: issues
- completed: issue comments & reactions
- skipped: pull requests: create, review comments, reviews — requires featureSha
- blocked: pull requests: update-branch & close — fixture step git/create-ref returned 405: 
- skipped: pull requests: merge — requires pr
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

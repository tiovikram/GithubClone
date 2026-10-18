# Security review

This review covered REST (`api/*`), GraphQL (`graphql/*`), git smart HTTP (`git/http.rs`), SSH (`ssh/*`), releases/uploads, webhooks/jobs, `/api/forge` and the web routes.
- Regression tests: `crates/forge/tests/security.rs` (#1-#17) and `crates/forge/tests/security2.rs` (#18-#23), plus unit tests in `src/security.rs`.
- Shared helpers: `crates/forge/src/security.rs`, `crates/forge/src/graphql/archived.rs`.

Severity reflects impact on a multi-tenant instance where anyone can sign up.

## Fixed

| # | Severity | Finding | Fix | Test |
|---|---|---|---|---|
| 1 | **Critical** | **Git option injection through GraphQL commit oids, giving arbitrary file write.**<br>`node(id:)` accepted `Commit:<repo>:<anything>`, and `GCommit` ran `git log -1 <oid>`. An oid of `--output=/path` wrote the attacker's commit log to any path the server can write (`authorized_keys`, other repos' caches …).<br>Any logged-in user could do this with any public repo. | The oid must be a 40-hex sha in `resolve_node` and `GCommit::info`. `git log` also gets `--end-of-options`. | `graphql_commit_node_id_cannot_inject_git_options` |
| 2 | **Critical** | **Stored variant of #1.** A review `commit_id` (REST, or GraphQL `addPullRequestReview(commitOID:)`) was stored as given. Later `PullRequestReview.commit` queries passed it to `git log`. | `create_review` and `locate` accept full shas only (422 otherwise). | `review_commit_id_must_be_a_sha` |
| 3 | **High** | **Webhook SSRF that returns the response.** Any user (repo admin of their own repo) could point a hook at `127.0.0.1`, `169.254.169.254` or cluster services, then read the stored response headers and body from `/hooks/{id}/deliveries/{d}`. | The webhook client uses `PublicOnlyResolver`, which drops loopback, RFC 1918, link-local, CGNAT, ULA and IPv4-mapped/NAT64 addresses at connect time, so DNS rebinding is covered. Literal IP and `localhost` URLs are refused before sending. Redirects stay disabled. The response body read is capped at 64 KiB while streaming.<br>Opt out with `FORGE_WEBHOOK_ALLOW_PRIVATE=1`. | `webhooks_do_not_reach_internal_addresses`, `ssrf_address_classification` |
| 4 | Medium | **Server-managed refs were writable.** Pushes over HTTP and SSH, and the git refs API, could create, move or delete `refs/pull/*`. That changes what a PR shows without touching the PR branch. | receive-pack runs with `-c receive.hideRefs=refs/pull` (HTTP and SSH). The git refs API returns 422 for `refs/pull/*`. | `pushes_and_api_cannot_touch_pull_refs` |
| 5 | Medium | **No login brute-force protection.** Applies to `/api/forge/session` and to HTTP Basic `login:password` on the API and git. There was also a user-enumeration timing oracle. | Failed attempts are counted (in Postgres since #21): 10 per (login, client IP) and 100 per client IP, per 15 min. After that the endpoints return 429 without checking the password. Unknown users pay for a dummy argon2 verification.<br>The client IP comes from connect info (`forge serve` now uses it), or the last `X-Forwarded-For` entry when `FORGE_TRUST_X_FORWARDED_FOR=1`. | `login_is_throttled_after_repeated_failures` |
| 6 | Medium | **Login CSRF on `/api/forge`.** The lenient JSON extractor accepts `text/plain` bodies, so a cross-site form could log the victim into an attacker's account, sign them up, or log them out. | Non-GET `/api/forge/*` requests must carry `X-Forge-CSRF` (the SPA always sends it) unless they use an `Authorization` header. | `forge_session_endpoints_require_csrf_header` |
| 7 | Medium | **Activity feeds leaked.**<br>(a) Draft releases (name, body, assets) appeared in `/events` and `/users/{u}/events`.<br>(b) Events of a repo made private later stayed public, because the `public` flag is fixed when the event is written. | Draft `release` events are not recorded as activity. Public feeds now also check the repo's *current* visibility. | `draft_releases_and_private_repos_stay_out_of_public_events` |
| 8 | Medium | **Archived repos were not read-only over REST.** Issues, comments, labels, PR merges, releases and other writes all worked. | REST middleware (`security::archived_read_only`, on `/api/v3` and `/api/uploads`) returns GitHub's 403 for mutating `/repos/{o}/{r}/…` requests. Still allowed: repo settings/unarchive/delete, subscription, forks, hooks, keys, collaborators, invitations, transfer.<br>Callers who can't see the repo still get 404. | `archived_repositories_are_read_only_over_rest` |
| 9 | Medium | **GraphQL resource exhaustion.** There was no depth or complexity limit, and `nodes(ids:)` was unbounded. | `limit_depth(32)` and `limit_complexity(5000)` (gh and introspection stay well below these). `nodes` is capped at 100 ids. | `misc_bounds` |
| 10 | Medium | **Unbounded request bodies.**<br>- Git HTTP bodies were unbounded, including gzip bombs.<br>- Release asset uploads were unbounded.<br>- Gist list views loaded every file's full content into memory. | - upload-pack request bodies are capped at 64 MiB after decoding.<br>- Pushes are capped by `FORGE_MAX_PUSH_BYTES` (default 2 GiB, HTTP and SSH).<br>- Assets are capped by `FORGE_MAX_ASSET_BYTES` (default 2 GiB).<br>- Gist lists select only name and `octet_length`. | `raw_content_is_not_rendered_by_browsers` (gist size); body caps by inspection |
| 11 | Low | **Draft (PENDING) reviews were readable by anyone** via `node(id:)`. | Only the author can resolve them. | `pending_review_not_readable_by_node_id` |
| 12 | Low | **User content served without hardening headers.** Covers raw files, `vnd.github.raw/html/diff` media types, release assets (uploader-chosen `Content-Type`) and gist raw. | Every response gets `X-Content-Type-Options: nosniff`. User-content responses also get `Content-Security-Policy: default-src 'none'; style-src 'unsafe-inline'; sandbox`. Assets are always `Content-Disposition: attachment`. | `raw_content_is_not_rendered_by_browsers` |
| 13 | Low | **Unvalidated tree paths.**<br>- `git mktree -z` input was built from tree paths that could contain NUL/control characters, injecting extra tree entries.<br>- `.GIT`/`.Git` (case variants of `.git`) were accepted through the trees and contents APIs. | Both are rejected with 422. | `tree_and_contents_paths_reject_dot_git_and_control_chars` |
| 14 | Low | **More user revisions reached git unchecked.**<br>- `?ref=` on `/license` went to `ls-tree` as an option.<br>- `plumbing::rev_parse` (release `target_commitish`, commit comments) did not use `--end-of-options`. | `safe_rev` checks plus `--end-of-options` on both. | `license_ref_is_not_an_option` |
| 15 | Low | **Cross-org team leak.** `parent_team_id` of another org's (secret) team showed that team's name and description. | Parents are loaded within the same org only. | `team_parent_from_other_org_is_not_exposed` |
| 16 | Low | **`cloneTemplateRepository` copied any readable repo**, not just templates. | The source must be a template. | `clone_template_requires_a_template` |
| 17 | Low | **Assorted bounds.**<br>- A huge `page` overflowed the offset (500 or panic).<br>- Code and commit search took any number of `repo:` qualifiers.<br>- Hooks per repo/org were unlimited (could starve the queue).<br>- `default_branch` on create was unvalidated (written into HEAD).<br>- Password login could match another account's email before the exact login. | - `page` is clamped.<br>- At most 20 repos per search.<br>- 20 hooks per repo/org (like GitHub).<br>- `default_branch` is validated.<br>- An exact login match wins over an email match. | `misc_bounds` |
| 18 | Medium | **GraphQL mutations on archived repos were not blocked** (the REST middleware of #8 does not cover GraphQL). | A schema extension (`graphql/archived.rs`) checks every top-level mutation field before it runs. It maps each node id in the arguments (literal or variable) to its repository: issues, PRs, comments, reviews, review comments, labels, milestones, releases, discussions, linked branches, refs/commits, rulesets, and repositories. Projects count too, through the content id in `addProjectV2ItemById` and the repository in `linkProjectV2ToRepository`. If one of those repos is archived and the viewer can see it, the field returns `null` with GitHub's error (`type: UNPROCESSABLE`, "Repository was archived so is read-only."). New mutations are covered automatically.<br>Still allowed, matching REST: `updateRepository`, `archiveRepository`/`unarchiveRepository`, `addStar`/`removeStar`, `updateSubscription` and `cloneTemplateRepository` (only reads the source). Project-owned data (item field values, draft issues, removing items) stays writable.<br>Repos the viewer can't see fall through to the mutation's normal "not found". | `graphql_mutations_on_archived_repositories_are_rejected` |
| 19 | Medium | **REST/GraphQL permission deviations.**<br>(a) Triage users could edit others' PR title, body and base.<br>(b) Repo admins could delete other users' reactions.<br>(c) `PATCH /issues/{n}` on a PR bypassed the PR rules, so a merged PR could be reopened.<br>(d) `addReaction` (GraphQL), reactions on issue comments, and reviews / review comments / review submission skipped the locked-conversation check. | (a) Title, body, base, `maintainer_can_modify` and draft changes need the author or Write. Close/reopen still allows Triage. Resending unchanged values is not an edit. This applies to REST `PATCH /pulls`, `PATCH /issues` and GraphQL `updatePullRequest`.<br>(b) Only the reaction's author may delete it (403 otherwise). This is the shared reaction service, used by issues, comments, commit comments and releases.<br>(c) GitHub accepts PR state changes through the issues endpoint, so forge does too, but routes them through `pulls::close_pull`/`reopen_pull`. A merged PR, or one whose head branch was deleted, gets GitHub's 422. Title and body follow (a). PRs get no `state_reason`. GraphQL `reopenIssue`/`closeIssue` on a PR follow the same path.<br>(d) On a locked conversation, only users with Write can react (GraphQL and REST, including comments), review, submit a pending review, or add review comments. | `triage_cannot_edit_other_users_pull_requests`, `only_the_author_can_delete_a_reaction`, `issues_endpoint_applies_pull_request_rules`, `locked_conversations_block_reactions_and_reviews` |
| 20 | Medium | **`POST /admin/organizations` let any user create an org** and name another user as its admin. | Site admins only (403 otherwise), as on GHES. Normal users use the forge-only `POST /api/forge/orgs` `{login, profile_name?}`, which always makes the caller the admin. Operators can also run `forge admin create-org --login L --owner U` (the gh suite uses this first). | `admin_organizations_is_for_site_admins` |
| 21 | Low | **The login throttle was per pod** (in memory), so N replicas allowed N times the limit. | The counters are in Postgres (`login_throttle(key, window_start, count)`, migration 0970). Each attempt costs one indexed read. A failure costs one atomic upsert for both keys (sorted, so no deadlocks; an expired window restarts at 1). A success costs one delete. About 1 in 64 failures also prunes expired rows. Logins in keys are capped at 256 chars. | `login_throttle_is_shared_between_replicas` (two servers, one database) |
| 22 | Low | **Free-text issue search with no repo qualifier was an ILIKE scan** over every visible issue. | Migration 0970 adds `pg_trgm` GIN indexes on `issues.title`, `issues.body` and `issue_comments.body`, plus `issues(number)`. Without a `repo:`/`user:`/`org:` qualifier, each word becomes `i.id IN (… UNION …)`, one indexed arm per field (title, body, the existing full-text index, comments with `in:comments`, issue number), so the same matches are found through indexes. Scoped searches keep the old form. On 200k issues, an unscoped word search runs in about 20 ms using bitmap index scans. Words shorter than 3 characters can't use trigrams. | `unscoped_issue_search_keeps_semantics` |
| 23 | Low | **Diff and patch output was generated in memory without a byte cap.** Covers `.diff`/`.patch` for commits, compare (including unrelated histories) and PRs, plus the patches inside commit/compare/PR-files JSON. | `FORGE_MAX_DIFF_BYTES` (default 10 MiB): git's stdout is read up to the cap, and git is killed past it. Past the cap:<br>- `.diff`/`.patch` responses return 406 `{"errors":[{"resource":…,"field":"diff","code":"too_large"}]}`, like GitHub's "diff too large".<br>- Commit/compare JSON keeps its complete file list and stats but leaves out `patch`.<br>- The PR file list is cut at the last complete file (GitHub also truncates huge PR file lists). | `diff_output_is_capped` |
| 24 | Medium | **Listings and search leaked private org repositories to members without access.** With the org base permission `none`, a member with no team, role or collaborator grant still saw a private repo's name and metadata (and its issues) in `/orgs/{o}/repos`, `/user/repos`, `/users/{u}/repos`, `/issues`, `/orgs/{o}/issues`, starred/watched lists, forks, search (repositories, issues, code, commits), GraphQL owner repository lists and issue search. Every filter had its own `EXISTS(org_members …)` copy. | One visibility rule for SQL: the Postgres function `forge_repo_readable(repo_id, user_id)` (migration 1201), used through `db::readable_sql` / `forge_repo_readable(r.id, $n)` by every list and search query. It mirrors `orgs::perm` / `db::permission`: public repos, site admins, the owner, collaborators, org owners, members when the base permission is not `none`, team grants (child teams inherit), and org roles with a repository base role (direct or through a team). The repos-area feeds and notification fan-out use the same rule. | `base_permission_none_hides_private_org_repos_from_lists_and_search` (repos_extra.rs: every list agrees with the direct permission check under base `none`, after a team grant, after removing it, and under base `read`) |

## Checked and OK
- **Objects addressed by global id are verified to belong to the repo in the path:**
  - comments, review comments, reviews, reactions, events, labels, milestones
  - releases and assets, hooks and deliveries, deploy keys, deployments and statuses, commit comments
  - GraphQL label/milestone/comment inputs
- **Private repos return 404 to anonymous users and non-collaborators** on every REST read path.
  - REST listings (`/user/repos`, `/users/{u}/repos`, `/orgs/{o}/repos`, `/issues`, starred/watched, forks, search issues/repos/topics/code/commits) filter visibility in SQL.
  - GraphQL `repository`, `node`/`nodes`, `search` and owner repository lists go through the repo loader.
- **SQL:** every `format!`/`AssertSqlSafe` interpolates only constants or whitelisted column names; user values are bound.
- **CSRF on the API:**
  - Cookie-authenticated mutating `/api/v3` and GraphQL requests without `X-Forge-CSRF` are treated as anonymous.
  - There is no CORS layer, and GraphQL is POST-only.
- **Sessions and passwords:**
  - Session cookie is `HttpOnly; SameSite=Lax` (`Secure` on https), stored as sha256, and fresh on every login (no fixation).
  - Tokens are stored as sha256.
  - Argon2id with default parameters (m=19 MiB, t=2).
- **Webhook secrets are masked** in API output, and signature headers are not stored.
- **Git access:**
  - HTTP and SSH enforce read for fetch and write for push.
  - Archived repos reject pushes.
  - Deploy keys are scoped to their repo and read-only unless created otherwise.
  - Repo paths come from DB ids, never from URL names. Names like `..` are rejected at creation, and the SSH path parser only allows `[A-Za-z0-9._-]`.
- **Most user revisions go through `read::safe_rev` and `--end-of-options`**, and commit-search words follow `-e`.
- **GitHub App credentials** (apps area, `crates/forge/tests/apps.rs`):
  - App private keys are generated server-side and returned once; only public keys are stored.
    JWTs must be RS256 (no `none`/HMAC), verify against a stored key, and live ≤ 10 minutes.
  - Client secrets are stored as sha256 and compared in constant time; app webhook secrets are
    sealed with the master key (`actions_settings::crypto`).
  - Installation tokens (`ghs_`) and user-to-server tokens (`ghu_`) are confined to their
    repositories and permissions on every surface that takes a token (REST, uploads, GraphQL, git
    smart HTTP, web routes) by one middleware; the bot user never has more than public read
    outside a token scope (`restrict` is fail-closed). Writes beyond the grant get 403 "Resource
    not accessible by integration"; private repositories outside the scope stay 404.
  - App tokens cannot reach `/api/forge` (no minting PATs), `/user` (bots), or the legacy
    authorizations API, so they cannot be escalated into broader credentials. Manifest codes are
    single-use, hashed and expire after 1 hour.
- **Migrations**: archive downloads use short-lived HMAC-signed URLs (`signing_secrets`
  `migration`); locked repositories refuse REST writes, pushes (HTTP, SSH) and GraphQL
  mutations (the `graphql/archived.rs` extension maps node ids to repositories; `type:
  FORBIDDEN`, "Repository has been locked for migration."), answering 404 / not found instead of
  the lock message to callers who can't see the repository.

## Known gaps / accepted risks (not fixed)
- **App tokens and GraphQL:** installation and user-to-server tokens get read-only GraphQL (forge
  can't map arbitrary mutations to app permissions yet).
- **Token scopes:** every token carries all scopes (documented in ARCHITECTURE). There are no fine-grained or read-only tokens.
- ~~**Org members get Write on every org repo.**~~ Fixed by the orgs build-out: `PATCH /orgs/{org}`
  `default_repository_permission` (`read` default, `write`, `admin`, `none`) is honoured by
  `db::permission` (via `orgs::perm`), together with team grants (inherited by child teams), custom
  repository roles, organization roles (`all_repo_*`, `security_manager`, custom base roles) and
  collaborator grants. Members who create or fork a repo into the org get admin on it, like on
  GitHub. Tests: `crates/forge/tests/orgs_extra.rs`.
  The listing/search side of this (members seeing private repos they can't access) is #24.
  Still open: issue assignability (`push_assignable`) and PR reviewer eligibility
  (`pulls::is_collaborator`) count any org member, regardless of the base permission.
- **Unverified emails** drive commit attribution (`users_by_email`), and `simple-user` JSON includes `email`, so `/users` lists every email. GitHub requires verified emails and exposes only public ones. This needs an email-verification feature.
- **The repo advisory lock is held while a slow client uploads its pack** (HTTP and SSH push). A slow pusher blocks other writes to that repo until the request ends or times out. Bounded by `FORGE_MAX_PUSH_BYTES` and the SSH inactivity timeout (1 h). A future fix is to receive into a quarantine first, then lock.
- **The timeline loads all items before paging** (diff/patch output is capped since #23).
- **Pages share the forge's origin** (`{base}/pages/{owner}/{repo}/`, GHES's path-based layout
  without subdomain isolation). Site files are served with
  `Content-Security-Policy: sandbox allow-scripts allow-forms allow-popups allow-modals allow-downloads`,
  so page scripts run in an opaque origin and can't read forge cookies or call the API with the
  visitor's session. A separate Pages domain would be stronger.
- **Pages artifact deployments** (`POST /pages/deployments`) only fetch artifacts stored on this
  forge (release assets the caller can read), never arbitrary URLs (no SSRF). The `oidc_token` is
  only checked for presence until forge has an OIDC issuer.
- **Webhooks honour system proxy settings** (`HTTPS_PROXY`). If a proxy is configured, the SSRF guard relies on the proxy's own egress policy.

## Enterprise administration (enterprise area)
- **Site admins are the enterprise owners.** Every `/admin/*`, `/enterprise/*`, `/enterprises/{e}/*`,
  `/manage/v1/*` and `/scim/v2/enterprises/{e}/*` endpoint requires `users.site_admin`
  (`enterprise::SiteAdmin`, 403 otherwise; 404 for an enterprise slug other than
  `FORGE_ENTERPRISE_SLUG`). Organization and repository pre-receive overrides follow GHES:
  org owners / repo admins, and only where the global hook allows downstream configuration.
- **Suspended users cannot authenticate**: token, Basic (token or password), session cookie and
  SSH key lookups all filter on `users.suspended_at` (`auth.rs`, `ssh/keys.rs`); suspension also
  deletes the user's sessions. Site admins must be demoted before they can be suspended or deleted.
- **Impersonation tokens** (`POST /admin/users/{u}/authorizations`) are ordinary tokens for the
  user (`tokens.impersonation = true`), audited (`oauth_access.create`) and revoked with
  `DELETE /admin/users/{u}/authorizations`.
- **Maintenance mode** (`POST /manage/v1/maintenance`) answers 503 to every request (API, git,
  web UI) except site admins, addresses on the IP exception list, `/healthz`, `/readyz`, the
  session endpoint (so admins can sign in) and `/api/v3/manage/*`. The state is read from Postgres
  by every pod (cached 2 s).
- **Repository invitations** are a site setting (`PUT /manage/v1/config/settings`
  `{"repository_invitations": {"enabled": true}}`, default `FORGE_REPO_INVITATIONS`): off, a repo
  admin's `PUT /collaborators/{u}` adds the user directly (GHES); on, it creates an invitation the
  invitee accepts or declines (`/user/repository_invitations`). Settings writes are a row-locked
  read-modify-write, so concurrent changes to different keys don't overwrite each other.
- **Announcement banner**: `GET /api/forge/announcement` (public, like the GHES banner shown on
  every page) returns the current `/enterprise/announcement` with its markdown rendered through
  the sanitizing renderer of `POST /markdown`; the web UI shows it above every page.
- **Pre-receive hooks: trust model.** Hook scripts and environments are **site-admin code**: only
  site admins can create or change hooks, choose their script repository/path or create
  environments; org owners and repo admins can only change the enforcement of hooks that allow
  it. During a push (HTTP and SSH) forge reads the script from the script repository's default
  branch and runs it through `forge pre-receive-hooks` as git's `pre-receive` hook, before the
  storage commit, so a rejected push never becomes durable. The runner is a limit, not a security
  boundary against a malicious admin:
  - the script gets a scrubbed environment: only `PATH`, `HOME` (a private per-push temp dir),
    `LANG`, git's object/quarantine variables (`GIT_DIR`, `GIT_QUARANTINE_PATH`,
    `GIT_OBJECT_DIRECTORY`, `GIT_ALTERNATE_OBJECT_DIRECTORIES`, push options) and GitHub's
    `GITHUB_USER_LOGIN`, `GITHUB_USER_IP`, `GITHUB_REPO_NAME`, `GITHUB_REPO_PUBLIC`, `GITHUB_VIA`;
    no forge secrets, database URL or cloud credentials are passed;
  - it runs in its own process group with a 5 s wall-clock limit (the whole group is killed on
    timeout) and its output is capped at 64 KiB per stream;
  - an environment is a `.tar.gz` downloaded (by a job, from an http(s) `image_url`; private
    addresses are refused unless `FORGE_WEBHOOK_ALLOW_PRIVATE=1`, max 512 MiB) into object storage
    and extracted per pod into the git cache dir; the script runs with that directory as its
    working directory and its `bin` dirs first on `PATH`. **This is not a chroot or container:**
    the script runs as the forge user with the pod's filesystem and network visible, like the
    branch-protection hook. Deployments that need isolation should run forge pods with a
    restricted service account / read-only root filesystem and network policies.
  - `testing` hooks run and print their verdict but never reject.
- **LDAP endpoints store mappings only**; no LDAP server is contacted (sync requests are recorded
  in `ent_ldap_sync_requests`). **Audit log streams** are stored (secrets must be sealed with the
  stream key and are verified, never decrypted into responses) but forge does not deliver events
  to external vendors. **Live migrations** are recorded with GHES's state machine; forge transfers
  no data to the target.

## GitHub Actions (runner area)
Workflow jobs run arbitrary code from anyone who can push a workflow file, or open a pull
request when a workflow runs on `pull_request`.
- **Never in the API server.** `forge serve` only plans jobs. Jobs run in `forge runner`, a
  separate process or pod (`deploy/base/forge-runner.yaml`). It talks to forge over HTTP only
  (`/api/forge/runner/*`), and it gets no database, object storage or Kubernetes credentials.
  With the shell executor the runner pod or host is the sandbox: one pod per job slot, an
  `emptyDir` work dir, non-root, a read-only root filesystem and no service account token.
  Every step process gets a scrubbed environment: `PATH`, `HOME` (a per-job directory), locale,
  the `GITHUB_*`/`RUNNER_*` variables, `CI`, `GIT_CONFIG_NOSYSTEM=1` and the job's own env. The
  runner's own variables, such as its credential, are never passed. Each step runs in its own
  process group, and the whole group is killed on cancel or timeout.
- **Credentials.**
  - *Registration tokens* (REST, valid 1 h, stored as sha256) register a runner for one
    repository, organization or the enterprise. They return a *runner credential* (`fgr_…`,
    sha256 stored). The runner credential can only claim jobs and report on jobs its runner
    holds.
  - *Job tokens* (`GITHUB_TOKEN`) are installation access tokens (`ghs_…`) of the built-in
    `github-actions` app, as on GitHub. One is minted per job when the job is claimed. It
    covers only the job's repository and acts as `github-actions[bot]`. Its permissions come
    from the job's or workflow's `permissions:` (GitHub's keys, `read-all`/`write-all`).
    Without `permissions:` they come from the repository's default workflow permissions:
    permissive (everything write) or restricted (contents/packages read, the default). Fork
    pull requests get read-only tokens unless the fork policy allows write. The apps area
    enforces the token the same way on REST, GraphQL and git smart HTTP (`apps::scope`, 403
    "Resource not accessible by integration"). The token is revoked when the job completes,
    and expires at the latest at the job timeout + 30 min. The app's installations are hidden
    from installation listings.
  - Events caused by `github-actions[bot]`, such as a push made with `GITHUB_TOKEN`, never
    trigger workflows (GitHub's recursion guard).
- **Secrets.** Secrets reach a job only through `runner::settings::secrets_for_job`
  (actions-settings). They go into the job message for the claiming runner. The runner masks
  every secret value, every line of a multi-line secret and every `::add-mask::` value in logs,
  and it masks the job token and its base64 auth header. Job outputs that contain a secret are
  dropped, as on GitHub. Pull requests from forks follow the fork policy
  (`fork_pr_policy`): workflows are skipped in private repositories unless allowed, the token is
  read-only and secrets/variables are withheld unless the policy sends them.
- **Actions policy.** The server checks `actions_allowed` (no runs at all when disabled),
  `action_allowed` for every top-level `uses:` (the runner fails a disallowed step),
  `self_hosted_runners_allowed` and runner groups (`runner_group_allows`) when it claims a
  job. It also checks environment branch policies, required reviewers, wait timers and custom
  protection rules before a job is queued.
- **`uses:`** never downloads code from github.com. `actions/checkout`, `upload-artifact`,
  `download-artifact` and `cache` are built into the runner. Other actions must be composite
  actions in repositories on this forge, fetched with the job token (so private repositories
  are not reachable). Node and docker actions fail with a clear message.
- **Downloads.** Run logs, job logs and artifacts are served after a 302 to a short-lived
  signed URL (HMAC with the server secret, bound to repository and object, 5 minutes), like
  GitHub's blob-storage redirects. Artifact downloads require authentication even on public
  repositories (GitHub does the same). Uploads stream to object storage and are capped by
  `FORGE_ACTIONS_MAX_UPLOAD` (2 GiB).
- **Known gaps.**
  - No per-job kernel isolation with the shell executor: jobs on the same runner host can see
    each other's processes. Use one slot per pod (the shipped default), or the docker executor.
  - No OIDC ID tokens (`ACTIONS_ID_TOKEN_REQUEST_URL`); only the subject-claim settings are
    stored.
  - First-time-contributor approval for fork pull requests is not enforced, because there is
    no approval UI.

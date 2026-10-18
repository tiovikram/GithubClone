# Security review

This review covered REST (`api/*`), GraphQL (`graphql/*`), git smart HTTP (`git/http.rs`), SSH (`ssh/*`), releases/uploads, webhooks/jobs, `/api/forge` and the web routes.
- Regression tests: `crates/forge/tests/security.rs`, plus unit tests in `src/security.rs`.
- Shared helpers: `crates/forge/src/security.rs`.

Severity reflects impact on a multi-tenant instance where anyone can sign up.

## Fixed

| # | Severity | Finding | Fix | Test |
|---|---|---|---|---|
| 1 | **Critical** | **Git option injection through GraphQL commit oids, giving arbitrary file write.**<br>`node(id:)` accepted `Commit:<repo>:<anything>`, and `GCommit` ran `git log -1 <oid>`. An oid of `--output=/path` wrote the attacker's commit log to any path the server can write (`authorized_keys`, other repos' caches …).<br>Any logged-in user could do this with any public repo. | The oid must be a 40-hex sha in `resolve_node` and `GCommit::info`. `git log` also gets `--end-of-options`. | `graphql_commit_node_id_cannot_inject_git_options` |
| 2 | **Critical** | **Stored variant of #1.** A review `commit_id` (REST, or GraphQL `addPullRequestReview(commitOID:)`) was stored as given. Later `PullRequestReview.commit` queries passed it to `git log`. | `create_review` and `locate` accept full shas only (422 otherwise). | `review_commit_id_must_be_a_sha` |
| 3 | **High** | **Webhook SSRF that returns the response.** Any user (repo admin of their own repo) could point a hook at `127.0.0.1`, `169.254.169.254` or cluster services, then read the stored response headers and body from `/hooks/{id}/deliveries/{d}`. | The webhook client uses `PublicOnlyResolver`, which drops loopback, RFC 1918, link-local, CGNAT, ULA and IPv4-mapped/NAT64 addresses at connect time, so DNS rebinding is covered. Literal IP and `localhost` URLs are refused before sending. Redirects stay disabled. The response body read is capped at 64 KiB while streaming.<br>Opt out with `FORGE_WEBHOOK_ALLOW_PRIVATE=1`. | `webhooks_do_not_reach_internal_addresses`, `ssrf_address_classification` |
| 4 | Medium | **Server-managed refs were writable.** Pushes over HTTP and SSH, and the git refs API, could create, move or delete `refs/pull/*`. That changes what a PR shows without touching the PR branch. | receive-pack runs with `-c receive.hideRefs=refs/pull` (HTTP and SSH). The git refs API returns 422 for `refs/pull/*`. | `pushes_and_api_cannot_touch_pull_refs` |
| 5 | Medium | **No login brute-force protection.** Applies to `/api/forge/session` and to HTTP Basic `login:password` on the API and git. There was also a user-enumeration timing oracle. | Failed attempts are counted in memory: 10 per (login, client IP) and 100 per client IP, per 15 min, with at most 100k tracked keys. After that the endpoints return 429 without checking the password. Unknown users pay for a dummy argon2 verification.<br>The client IP comes from connect info (`forge serve` now uses it), or the last `X-Forwarded-For` entry when `FORGE_TRUST_X_FORWARDED_FOR=1`. | `login_is_throttled_after_repeated_failures`, unit `throttles_after_failures` |
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

## Known gaps / accepted risks (not fixed)
- **Token scopes:** every token carries all scopes (documented in ARCHITECTURE). There are no fine-grained or read-only tokens.
- ~~**Org members get Write on every org repo.**~~ Fixed by the orgs build-out: `PATCH /orgs/{org}`
  `default_repository_permission` (`read` default, `write`, `admin`, `none`) is honoured by
  `db::permission` (via `orgs::perm`), together with team grants (inherited by child teams), custom
  repository roles, organization roles (`all_repo_*`, `security_manager`, custom base roles) and
  collaborator grants. Members who create or fork a repo into the org get admin on it, like on
  GitHub. Tests: `crates/forge/tests/orgs_extra.rs`.
  **Remaining gap (for the security area):** listing/search SQL filters across `api/*` and
  `graphql/*` still treat any org membership as visibility of *private* org repositories
  (`EXISTS(SELECT 1 FROM org_members ...)`). With base permission `none`, a member without a team
  or collaborator grant can therefore see a private repo's *name/metadata in listings and search*
  (e.g. `/orgs/{o}/repos`, `/user/repos`, search), although every direct access path (`load_repo`,
  git, SSH, GraphQL `repository`/`node`) returns 404. Fix: replace those filters with a shared SQL
  predicate that mirrors `orgs::perm::org_repo_permissions`.
- **Unverified emails** drive commit attribution (`users_by_email`), and `simple-user` JSON includes `email`, so `/users` lists every email. GitHub requires verified emails and exposes only public ones. This needs an email-verification feature.
- **GraphQL mutations on archived repos** are not blocked (the REST middleware does not cover GraphQL).
- **REST permission deviations:**
  - Triage users can edit others' PR title/body/base. GitHub requires the author or Write.
  - Admins can delete other users' reactions.
  - A PR can be reopened through `PATCH /issues/{n}`.
  - `addReaction` (GraphQL) and some review paths skip the locked-conversation check.
- **The repo advisory lock is held while a slow client uploads its pack** (HTTP and SSH push). A slow pusher blocks other writes to that repo until the request ends or times out. Bounded by `FORGE_MAX_PUSH_BYTES` and the SSH inactivity timeout (1 h). A future fix is to receive into a quarantine first, then lock.
- **Diff and compare output** (`diff_text`, `format_patch`, compare across unrelated histories) is generated in memory without a byte cap. Commit listings are capped at 300 files for JSON. The timeline loads all items before paging.
- **Free-text issue search** without a repo qualifier is an ILIKE scan over all visible issues.
- **The login throttle is per pod** (in memory). With N replicas the effective limit is N× the configured one.
- **Webhooks honour system proxy settings** (`HTTPS_PROXY`). If a proxy is configured, the SSRF guard relies on the proxy's own egress policy.
- **`POST /admin/organizations`** lets any user create an org (and name another user as its admin).

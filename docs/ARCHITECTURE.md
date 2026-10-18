# forge — architecture contract

forge is a GitHub-compatible code forge in Rust. This file is the contract shared by everyone
working on it. Change it deliberately.

## Oracles (how we are judged)
1. Stock `git` clone/fetch/push over smart HTTP (later SSH) against forge.
2. Stock `gh` CLI configured for forge as an enterprise host: % of the command suite in
   `tests/gh/` that passes.
3. GitHub REST OpenAPI description: % of operations conforming (`tools/conformance`).
4. Beat Forgejo/Gitea on GitHub-API compatibility and on k6 performance.
5. Portable: Postgres + S3-compatible object storage (MinIO) + Kubernetes only. Same images and
   manifests run on k3d and kind. Pods are stateless and can be killed at any time.

## Process layout
One binary, `forge` (crate `forge` in `crates/forge`):
- `forge serve` — HTTP server (REST, GraphQL, git smart HTTP, web UI). Stateless.
- `forge runner` — GitHub Actions runner (separate process/pod; registers with a registration
  token, long-polls `/api/forge/runner/acquire`, runs jobs with the shell or docker executor).
  Jobs never run inside `serve`; `serve` only plans them (and runs the cron scheduler under an
  advisory lock). See docs/deploy.md (runners) and docs/security.md (Actions).
- `forge migrate` — run SQL migrations (also run automatically on `serve` unless `FORGE_SKIP_MIGRATIONS=1`).
- `forge admin create-user --login L --password P [--admin]`, `forge admin create-token --login L`.

Configuration is environment only:
| var | meaning | default |
|---|---|---|
| `FORGE_LISTEN` | bind address | `0.0.0.0:3000` |
| `FORGE_BASE_URL` | external URL used in API responses (`html_url`, `url`, clone URLs) | `http://localhost:3000` |
| `DATABASE_URL` | Postgres URL | `postgres://forge:forge@localhost/forge` |
| `FORGE_STORAGE` | `file:///abs/path` or `s3://bucket/prefix` | `file://$PWD/data/objects` |
| `AWS_ENDPOINT_URL`, `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`, `AWS_REGION`, `AWS_ALLOW_HTTP` | S3 settings (MinIO) | |
| `FORGE_CACHE_DIR` | local, disposable git repo cache | `$PWD/data/cache` |
| `FORGE_GIT_BIN` | git executable | `git` |
| `FORGE_UNIX_SOCKET` | also serve HTTP on this unix socket (gh tests use `http_unix_socket`) | unset |
| `FORGE_SSH_LISTEN` | start the git-over-SSH server on this address (e.g. `0.0.0.0:2222`) | unset (no SSH) |
| `FORGE_SSH_HOST_KEY` | OpenSSH private host key file | unset = shared ed25519 key in Postgres |
| `FORGE_SSH_URL_HOST`, `FORGE_SSH_URL_PORT` | host (`host` or `host:port`) / port used in `ssh_url` | base URL host, `FORGE_SSH_LISTEN` port |
| `FORGE_ENTERPRISE_SLUG` | slug of the single enterprise (`/enterprises/{slug}`, SCIM); site admins are its owners | `forge` |
| `FORGE_HOOK_EXEC_BIN` | binary that runs enterprise pre-receive hooks (`forge pre-receive-hooks`) | the running `forge` executable |
| `FORGE_SECRET_KEY` | master key (any string; X25519 secret = sha256 of it) sealing the Actions secret private keys; same value on every pod | unset = random master key stored in `signing_secrets` |
| `FORGE_HOOK_BIN` | forge executable that the `pre-receive` hook runs for secret scanning push protection | the running binary when it is named `forge` |

## URL layout (mirrors GitHub Enterprise Server)
- `/api/v3/...` — REST API (GitHub shapes).
- `/api/graphql` — GraphQL (subset of GitHub schema needed by gh; see docs/research/gh-protocol.md).
- `/{owner}/{repo}(.git)?/info/refs`, `/git-upload-pack`, `/git-receive-pack` — git smart HTTP.
- `/api/forge/...` — forge-only endpoints (signup, login/session, admin). Not GitHub API.
- `/v2/...` — OCI container registry (GHCR equivalent; images `{host}/{owner}/{name}`), token
  endpoint `/v2/token`. `/npm/...` — npm registry for `@owner/name`. See "Packages" below.
- `/login`, `/logout` handled by the SPA + `/api/forge/session`.
- everything else — the web UI (SPA, embedded static assets, `index.html` fallback).

## Auth
- Personal access tokens: `gho_` + 36 random base62 chars (gh accepts any opaque string). Stored as
  sha256 hex. Accepted as `Authorization: token X`, `Authorization: Bearer X`, or HTTP Basic with
  the token as password (any username; `x-access-token` ok). Basic `login:password` also works
  for git and the API.
- Web sessions: cookie `forge_session` (random, sha256 stored in `sessions`). Cookie-authenticated
  mutating API requests must send header `X-Forge-CSRF: 1` (SPA always does).
- Every authenticated API response includes `X-OAuth-Scopes: repo, read:org, gist, workflow, admin:org, user, delete_repo`
  and `X-Accepted-OAuth-Scopes`. All responses include `X-GitHub-Request-Id`, and
  `X-GitHub-Enterprise-Version` (we claim `3.14.0`).

### GitHub Apps and OAuth apps (`crate::apps`, `api/apps_*.rs`)
- **Registration** is forge-only: `POST /api/forge/apps` (a GitHub App manifest, optional `owner`
  org) or the manifest flow (`POST /api/forge/app-manifests` returns a one-time `code`, exchanged
  without auth at `POST /api/v3/app-manifests/{code}/conversions`). Both return the app with
  `client_secret`, `webhook_secret` and an RSA-2048 private key `pem` (PKCS#1), shown once: only
  the public key (`app_keys`) and a sha256 of the client secret are stored. App ids start at 2
  (id 1 is the built-in checks app). Each app has a bot user `<slug>[bot]` (kind `Bot`).
  `POST /api/forge/oauth-apps` registers an OAuth app (client id/secret only).
- **App JWT**: `Authorization: Bearer <jwt>`, RS256 signed by one of the app's keys, `iss` = app id
  or client id, `iat`/`exp` with `exp - iat` ≤ 10 min (60 s drift). Required by `/app`,
  `/app/installations*`, `/app/hook/*`, `/{orgs,users,repos,enterprises}/…/installation`.
  Failures are 401 with GitHub's messages.
- **Installations**: `POST /api/forge/apps/{slug}/installations` (own account, an org the caller
  owns, or `enterprise: true` for site admins) or `POST /enterprises/{e}/apps/organizations/{org}/installations`.
  Private apps install only on their owner's account. An installation copies the app's
  permissions/events and covers `all` or `selected` repositories of the account.
- **Installation access tokens** (`POST /app/installations/{id}/access_tokens`): `ghs_` + 36,
  sha256-stored, 1 h, optionally narrowed to repositories and a permission subset. They
  authenticate as the bot user and are confined by `apps::scope::middleware` (mounted on the
  whole app): the request is mapped to a GitHub App permission (`issues`, `contents`,
  `pull_requests`, `checks`, `administration`, …; git push = `contents:write`) and refused with 403
  "Resource not accessible by integration" when not granted; otherwise it runs in a task-local
  scope and `db::permission` → `apps::scope::restrict` gives covered repositories exactly the
  level the request needs and every other repository at most public read (private ones 404).
  `/user*`, `/api/forge`, gists and admin endpoints are refused. GraphQL is read-only for app
  tokens. Suspended installations' tokens stop working. Check runs/suites created with an
  installation token belong to that app (`app` in check JSON).
- **User-to-server tokens**: the device flow with a GitHub App's client id issues `ghu_` (8 h) +
  `ghr_` refresh (6 months; `grant_type=refresh_token` rotates both). They act as the user,
  restricted (min of user permission and the app's permission) to the app's installations'
  repositories. OAuth apps get classic `gho_` tokens with the requested scopes.
  `/applications/{client_id}/token*` (Basic `client_id:client_secret`) check, reset, delete and
  scope them.
- **Legacy OAuth Authorizations API** (`/authorizations*`, `/applications/grants*`): HTTP Basic
  only (`login:password`, or a classic token as the password; app tokens refused); creates
  classic tokens with scopes, grouped per user and application in `oauth_grants`.
- **App webhooks**: one `webhooks` row per app (`app_id`), delivered by the standard queue with
  `X-GitHub-Hook-Installation-Target-Type: integration`. Apps receive `installation` /
  `installation_repositories` events, and repository events of their installations' repos for
  subscribed events. The webhook secret is stored sealed with the app's key
  (`api::actions_settings::crypto`, master key).

## Data model
Postgres, migrations in `crates/forge/migrations` (sqlx). Ranges: `0001-0099` core (lead),
new migrations by feature owners use the next free number with a descriptive name; never edit a
merged migration.
IDs are `BIGINT` identity (GitHub uses integers). GraphQL node ids are opaque
`base64url("<Type>:<id>")` with a type prefix, see `crate::ids`.
Issues and pull requests share one per-repo number sequence (`repositories.next_number`);
a PR is an `issues` row with `is_pull = true` plus a `pull_requests` row.

## Git storage (the scalability core)
Source of truth for repository content is **object storage**, so any pod can serve any repo and
pods can be killed at any time:
- Each repo has `repo_storage(repo_id, version, refs, packs)` in Postgres: `refs` is the
  packed-refs text + HEAD, `packs` the list of pack names; object storage holds
  `repos/{repo_id}/packs/pack-<sha>.pack|.idx` (immutable, content-addressed).
- A pod keeps a disposable bare-repo cache at `$FORGE_CACHE_DIR/{repo_id}.git`, plus a
  `forge-version` file. Before reading, it syncs: download missing packs, rewrite refs, if
  local version < db version.
- Writes (push, API commits, merges) take a Postgres advisory lock on the repo id
  (`pg_advisory_lock(repo_id)` on a dedicated connection), sync, run the git operation with
  `transfer.unpackLimit=1` (so pushes always land as packs; API writes call
  `git repack -d` without `-a` to pack loose objects), upload any new packs **before** committing
  `repo_storage` (version+1, new refs, new pack list) in a transaction, then unlock.
  A crash before commit leaves orphan packs only (GC'd later); never a torn repo.
- `forge gc` (CronJob) repacks repos with many packs into one and deletes unreferenced packs.

Git operations shell out to the `git` binary (`git upload-pack/receive-pack --stateless-rpc`,
`git cat-file`, `git merge-tree --write-tree`, `git commit-tree`, `git diff`...). Code lives in
`crates/forge/src/git/`.

## SSH (`crates/forge/src/ssh/`)
An embedded `russh` server, started by `forge serve` when `FORGE_SSH_LISTEN` is set (k8s: one more
`Service` port; any pod can serve any connection, same as HTTP).
- **Host key.** `FORGE_SSH_HOST_KEY` (OpenSSH private key file, e.g. a mounted Secret) or, if
  unset, an ed25519 key in `ssh_host_keys` generated once by the first pod
  (`INSERT .. ON CONFLICT DO NOTHING`, then re-read). Every replica presents the same identity,
  so clients never see a host-key change when routed to another pod. The DB copy is stored
  unencrypted, so production should prefer the Secret.
- **Auth.** Public key only, username `git`. The key's SHA256 fingerprint (OpenSSH format,
  `SHA256:…`) is looked up in `ssh_keys` (user keys), then in `deploy_keys` (per repo, read-only
  unless created with `read_only=false`). A fingerprint can be registered only once across both
  tables (422 `key is already in use`).
- **Commands.** Exec `git-upload-pack`, `git-receive-pack`, `git-upload-archive` (the
  `git upload-pack` spelling works too). The path may be quoted or bare, with an optional leading
  `/` or `~/` and an optional `.git`. Any other command gets `Invalid command` and a shell request
  gets `Hi <login>! You've successfully authenticated, but forge does not provide shell access.`;
  both exit 1. Authorization follows the HTTP rules: `Perm::Read` for upload/archive,
  `Perm::Write` for receive, and deploy keys only on their own repo. Repos the caller can't see
  get `ERROR: Repository not found.`. `GIT_PROTOCOL` env requests are honoured (protocol v2 for fetch).
- **Reads** sync the cache, then run `git upload-pack .` full-duplex over the channel (no
  `--stateless-rpc`).
- **Push guarantee.** `git receive-pack .` runs inside `GitStore::write` (cluster-wide lock,
  pack upload, manifest commit). Over SSH, receive-pack would stream its report-status to the
  client before the closure returns, so before the manifest commit. Hooks can't fix this:
  `pre-receive` runs before the refs exist, and `post-receive` runs inside the very write it would
  have to commit. Instead forge interposes on receive-pack's stdout. The ref advertisement (up to
  the first flush-pkt) is forwarded live. Everything after it (report-status, sideband progress)
  is held in memory until `GitStore::write` has uploaded the packs and committed `repo_storage`.
  Only then does forge send the report, run `events::on_push` and send exit-status 0. If
  receive-pack or the commit fails, forge discards the held report. The client instead gets a
  synthesized report-status that rejects every ref it asked to update
  (`ng <ref> storage commit failed`, side-band wrapped if negotiated). forge builds that report
  from the client's command list, which it parses as it streams in. The client also gets an
  `ERROR:` line on stderr and exit-status 1. Guarantee: **a client never sees `ok` for a ref
  update that is not durably committed.** As with HTTP, a crash after the commit but before the
  report reaches the client can make a successful push look failed (retrying is safe). Also as
  with HTTP, the repo lock is held while the client uploads its pack.
- **`ssh_url`** in repository JSON is `git@host:o/r.git` on port 22, otherwise
  `ssh://git@host:port/o/r.git` (`ssh::ssh_url`; `build_state` registers the prefix per base URL).
- **REST**: `/user/keys`, `/user/keys/{id}`, `/users/{u}/keys`, `/repos/{o}/{r}/keys[/{id}]`
  (repo admin), and plain-text `GET /{login}.keys` (a middleware that runs before the router).

## Actions secrets and settings (`api/actions_secrets.rs`, `api/actions_settings/`)
- **Secrets.** Clients seal values with libsodium `crypto_box_seal` (X25519 + XSalsa20-Poly1305)
  for the owner's public key (`.../secrets/public-key`; environments use their repository's key).
  Each repository/org has an X25519 key pair in `actions_public_keys`; the private half is itself
  sealed with the master key (`FORGE_SECRET_KEY`, else a DB-stored random key) and tagged
  `v1:env:` / `v1:db:`. A PUT whose value does not open with the current key is a 422. Values are
  decrypted only by `actions_settings::secrets_for_job(state, repo_id, env)` (environment >
  repository > organization; org secrets by visibility `all` / `private` / `selected`).
  `variables_for_job` is the same for `vars.*`.
- **Policies** (`actions_policies`, one row per repo / org / the enterprise, owner id 0): Actions
  enabled, allowed actions + patterns, SHA pinning, default `GITHUB_TOKEN` permissions, PR
  approval by workflows, retention, fork PR settings, workflow access, self-hosted runner access,
  OIDC `sub` template. A level can be at most as permissive as an explicit setting above it.
  The runner calls `actions_allowed`, `workflow_token_permissions`, `action_allowed`,
  `fork_pr_policy`, `retention_days`, `self_hosted_runners_allowed`, `workflow_access_allowed`,
  `oidc_sub_claim_keys`, `runner_group_of`, `runner_group_allows`.
- **Runner groups** (`actions_runner_groups`): per org and for the enterprise, with a `Default`
  group each; membership in `actions_runner_group_runners` (no row = default group). forge has no
  GitHub-hosted runner fleet: jobs run on self-hosted runners (`forge runner`).

## Packages (`packages/`, `api/packages.rs`)
- **Storage.** Every package object (layer, image config, manifest, npm tarball) is a blob at
  `packages/blobs/sha256/<hex>` in object storage, shared between packages (`package_blobs`).
  Postgres holds `packages` (owner, type `container`/`npm`, name, visibility, linked repo),
  `package_versions` (container: name = manifest digest; npm: the version), `package_tags`
  (container tags, npm dist-tags), `package_version_blobs` (what each version references) and
  `package_repo_blobs` (blobs pushed into `owner/name` before a manifest references them).
- **Uploads** (`package_uploads`): each PATCH chunk is its own object
  (`packages/uploads/<id>/<n>-<rand>`), appended with a compare-and-swap on the part count; the
  final PUT re-hashes (or, for a single chunk, reuses the hash), verifies the digest and copies
  into the blob store. Any pod can take any chunk.
- **Auth.** `/v2/` answers 401 + `WWW-Authenticate: Bearer realm="{base}/v2/token"`; the token
  endpoint (GET with Basic, or docker's POST `grant_type=password|refresh_token`) returns an
  HMAC-signed identity token (`fgr_<uid>.<exp>.<mac>`, key `signing_secrets.registry`). PATs and
  Basic work directly too. Permissions are checked per request (`packages::package_perm`).
- **Permissions** (GitHub's granular model): the owning user, the owners of the owning org and
  the publisher administer; a package linked to a repository (`org.opencontainers.image.source`
  label/annotation, package.json `repository`, same owner only) inherits its access (read/write/
  admin); public packages are world-readable. New packages are private unless linked to a public
  repository at creation. Publishing a new package needs the namespace (the user, or org
  membership).
- **Deletes are soft** (REST API, `DELETE manifests/<digest>`, `npm unpublish`); restorable for
  30 days; `forge gc` (`packages::gc::run`) purges them and deletes blobs nothing references
  (1 h grace, refreshed whenever a client is told a blob exists).

## Advanced security (`advsec/`, see docs/advsec.md)
- Features (secret scanning, push protection, Dependabot alerts, code scanning) act only when
  enabled per repository (`repo_security_products`, `repo_settings.vulnerability_alerts`); push
  protection is off by default.
- Scans run as Postgres jobs (`advsec.*` queues): secret scans of the commits a push adds (queued by
  `hooks::after_refs_changed`) and backfills; Dependabot matching of the default branch's
  dependency graph against the advisory database (`advisories`, imported from OSV records with
  `forge advisories sync`); SARIF processing (files in object storage under `advsec/sarif/`).
- Push protection runs inside `git receive-pack` as a `pre-receive` hook that executes
  `forge hook pre-receive` (no database access; a per-push rules file), chained after branch
  protection. Contents API writes are checked in-process (409 with placeholder ids).

## Crate layout (`crates/forge/src`)
```
main.rs          CLI entry
config.rs        env config
state.rs         AppState { db, store, cfg, git } (Clone, Arc inside)
error.rs         ApiError -> GitHub error JSON {message, documentation_url, errors?}
auth.rs          extractors: MaybeUser, AuthUser (401 if absent)
ids.rs           node id encode/decode
json/            GitHub JSON shape builders (user, repo, issue, pull, label, ...) — shared by REST & GraphQL
api/mod.rs       REST router (/api/v3), pagination helpers (Link header)
api/<area>.rs    one file per area: users, repos, contents, git_data, issues, comments, labels,
                 milestones, pulls, reviews, releases, search, meta, orgs, stars ...
graphql/         async-graphql schema (gh subset)
git/             storage sync, smart HTTP, plumbing helpers
ssh/             embedded SSH server (git over SSH), SSH/deploy key service layer
web.rs           SPA static assets (rust-embed of web/dist)
forge_api.rs     /api/forge endpoints (signup, session)
```
Every area module exposes `pub fn routes() -> Router<AppState>`, merged in `api/mod.rs`.

## JSON conventions
Match GitHub exactly: snake_case, ISO-8601 `Z` timestamps without fractional seconds, `null`
rather than omitted for nullable fields, full `url`/`html_url`/`*_url` templates built from
`FORGE_BASE_URL` (`{base}/api/v3/repos/o/r`, html `{base}/o/r`). Lists support
`page`/`per_page` (default 30, max 100) and emit `Link` headers with rel next/last/first/prev.
Errors: 404 `{"message":"Not Found","documentation_url":"https://docs.github.com/rest"}`;
422 `{"message":"Validation Failed","errors":[{"resource":"Issue","field":"title","code":"missing_field"}],"documentation_url":...}`.
Private repos the caller cannot see return 404, not 403.

## Testing
- `cargo test` — unit + integration tests (integration tests use `DATABASE_URL_TEST`, file storage in a temp dir).
- `tests/e2e/git.sh` — stock git clone/push/fetch.
- `tests/gh/run.sh` — gh command suite; prints pass %.
- `tools/conformance` — OpenAPI conformance score.
- `deploy/` — k8s manifests (kustomize), `tests/load` k6, `tests/chaos` pod-kill.

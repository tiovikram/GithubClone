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

## URL layout (mirrors GitHub Enterprise Server)
- `/api/v3/...` — REST API (GitHub shapes).
- `/api/graphql` — GraphQL (subset of GitHub schema needed by gh; see docs/research/gh-protocol.md).
- `/{owner}/{repo}(.git)?/info/refs`, `/git-upload-pack`, `/git-receive-pack` — git smart HTTP.
- `/api/forge/...` — forge-only endpoints (signup, login/session, admin). Not GitHub API.
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

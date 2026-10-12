# Team conventions (read before writing code)

You are one of several engineers working in parallel, each in your own git worktree. The lead
merges branches. Read `docs/ARCHITECTURE.md` (the contract) and skim the existing code first:
`crates/forge/src/{db.rs,auth.rs,error.rs,extract.rs,api/mod.rs,api/repos.rs,json/*.rs,git/*.rs}`.

## Toolchain
`source scripts/env.sh` puts cargo, psql on PATH. Postgres 16 runs natively on localhost
(user/password `forge`/`forge`, superuser). Docker works (Docker Desktop, 10 CPU / 8 GB).

## Isolation rules (important)
- **Your own databases.** Create and use `forge_<area>` (dev) and `forge_<area>_test` (tests):
  `createdb -O forge forge_<area>` and set `DATABASE_URL` / `DATABASE_URL_TEST`. Never use the
  shared `forge`/`forge_test` databases: other branches add migrations and sqlx will refuse.
- **Never `pkill -f "forge serve"` broadly**: kill only your own process (match your worktree path).
- **Your own port** for dev servers (given in your brief) via `FORGE_LISTEN=127.0.0.1:<port>`,
  `FORGE_BASE_URL=http://localhost:<port>`, and data dirs under your worktree's `data/`.
- **Never touch the user's git credentials/keychain.** Every `git` you run (scripts, tests) must
  have `GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_GLOBAL=/dev/null GIT_TERMINAL_PROMPT=0` (macOS system git
  config enables the osxkeychain helper which would store test tokens). The test harness
  (`crates/forge/tests/common/mod.rs`) already does this.
- **Migrations:** add new files only, numbered in your assigned range (e.g. `0200_issues_extra.sql`).
  Never edit `0001_core.sql`; if you need a schema change, add an ALTER in your range.
- **Shared files** (`lib.rs`, `api/mod.rs`, `json/mod.rs`, `hooks.rs`, `Cargo.toml`): keep edits
  minimal and additive (add a `pub mod` line, a `.merge(x::routes())` line, a call in a hook) so
  merges are trivial. Put real code in your own modules.

## Code conventions
- Use `crate::extract::Json` (lenient, GitHub-style errors), never `axum::Json`, for request bodies.
- Errors: return `ApiError` (`error.rs`); 404 for repos the caller can't see; 422 validation with
  GitHub's `errors` array; 409 for git conflicts.
- Lists: `PageParams` + `Paged` (Link headers). Batch-load related rows (no N+1 per item).
- JSON must match GitHub's OpenAPI schemas exactly (all required fields, correct nullability,
  `*_url` fields). Shape builders go in `json/` so REST and GraphQL share them.
- **Service layer:** every mutation is a `pub async fn` separate from the axum handler, e.g.
  `pub async fn create_issue(state: &AppState, ctx: &RepoCtx, actor: &User, input: CreateIssue) -> ApiResult<IssueRow>`.
  The handler only parses/authorizes/renders. GraphQL will call the same functions.
- Git writes go through `state.git.write(repo_id, |path| async { ... })` (cluster-wide lock,
  storage commit). Reads: `state.git.sync(repo_id)` then run git on the returned path.
  After a write that changes refs, call `crate::hooks::after_refs_changed`.
- Write integration tests in `crates/forge/tests/<area>.rs` using `tests/common/mod.rs`
  (`#[tokio::test(flavor = "multi_thread")]`). `cargo test` must pass before you finish.
- Validate against the oracle: `tools/conformance` (see docs/conformance.md):
  `cd tools/conformance && node run.mjs --base http://localhost:<port>/api/v3 --token <tok>`.
  Your operations must come out "conforming". Don't edit tools/conformance except to fix a
  genuine harness bug (say so in your report).
- Commit your work on your branch with clear messages (author `forge-<area>`), ending with
  `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. Do not push anywhere.

## Report
Finish with a short report: endpoints implemented, conformance numbers for your area, tests,
anything you changed in shared files, known gaps, and service functions others can call
(with signatures).

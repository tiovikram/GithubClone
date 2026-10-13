# forge web UI

Single-page app (Vite + React + TypeScript + react-router) for forge. It talks only to the
GitHub-compatible REST API under `/api/v3` and the forge-only endpoints under `/api/forge`
(see `docs/ARCHITECTURE.md`). The Rust server embeds `web/dist` and serves `index.html` for
every path it does not otherwise handle.

## Commands

```sh
npm install
npm run dev        # Vite on :5173, proxies /api and git smart-HTTP paths to http://localhost:3000
npm run dev:mock   # same, but proxies to the in-memory mock backend (mock/server.mjs on :3001)
npm run mock       # just the mock backend
npm run typecheck  # tsc --noEmit
npm run build      # typecheck + production build into web/dist
npm run e2e -- --base http://localhost:3109 --login NEWUSER --password 'some-pass-1'   # see below
```

`FORGE_BACKEND=http://host:port npm run dev` points the dev proxy at another backend.

Mock accounts: `octocat` / `password` and `hubot` / `password`. A seeded token
`gho_mocktokenmocktokenmocktokenmocktoken` belongs to octocat. Mock state is in-memory and is
reset on restart. `MOCK_LATENCY=0` removes the artificial 120 ms delay.

## Layout

```
src/api.ts          typed API client: request/requestPage/requestAll, ApiError (GitHub `message` + `errors`),
                    Link-header parsing, one function per endpoint
src/auth.tsx        current user context (GET /api/v3/user on load)
src/hooks.ts        useAsync (loading/error/data/reload), useTitle
src/markdown.tsx    marked + DOMPurify, highlight.js for fenced code
src/highlight.ts    highlight.js core with a small language set, per-line splitting for the file view
src/components/     layout, repo header and tabs, diff viewer, commit list, conversation (comments, labels)
src/pages/          one module per area (Home, Auth, Profile, NewRepo, Tokens, Keys, Search, Code, Commits, Issues, Pulls,
                    Releases, Refs (branches/tags), RepoSettings)
e2e/run.mjs         end-to-end flows in headless Chrome (puppeteer-core) against a running server
mock/server.mjs     in-memory mock backend (plain node:http)
mock/dev.mjs        runs the mock and Vite together
```

## Routes

| URL | Page |
|---|---|
| `/` | landing (signed out) / dashboard (signed in) |
| `/login`, `/signup` | session forms (`?return_to=` supported) |
| `/new` | create repository |
| `/settings/tokens` | personal access tokens, gh/git setup |
| `/{owner}` | profile + repositories |
| `/{owner}/{repo}`, `/tree/{ref}/{path}` | file browser, README, clone URL, empty-repo instructions |
| `/{owner}/{repo}/blob/{ref}/{path}` | file view with line numbers and highlighting |
| `/{owner}/{repo}/commits/{ref}` (`?path=`) | commit history |
| `/{owner}/{repo}/commit/{sha}` | commit diff |
| `/{owner}/{repo}/issues`, `/issues/new`, `/issues/{n}` | issues |
| `/{owner}/{repo}/pulls`, `/compare/{base}...{head}`, `/pulls/new` | pull requests, compare and create |
| `/{owner}/{repo}/pull/{n}`, `/pull/{n}/commits`, `/pull/{n}/files` | PR conversation, commits, diff, merge |
| `/{owner}/{repo}/settings` | rename, description, website, default branch, visibility, delete |
| `/{owner}/{repo}/branches`, `/tags` | branch list (compare, delete), tag list (source archives) |
| `/{owner}/{repo}/releases`, `/releases/new`, `/releases/tag/{tag}`, `/releases/latest` | releases, create with asset upload, view, download |
| `/settings/keys` | SSH keys (`/user/keys`) |
| `/search?q=&type=repositories\|issues\|code` | search (`/search/*`); the header box scopes to the current repo |
| `/{owner}?tab=stars` | starred repositories |

Server-owned paths that the SPA must never route (they are plain `<a href>` links in the UI):
`/{o}/{r}(.git)/info/refs`, `/git-upload-pack`, `/git-receive-pack`, `/{o}/{r}/archive/*`, `/{o}/{r}/raw/*`,
`/{o}/{r}/releases/download/*`, `/{o}/{r}/releases/latest/download/*`, `/{login}.keys`, `/avatars/*`, `/api/*`
(unknown `/api/*` paths return a JSON 404, not the SPA shell).

Refs that contain slashes (`feature/x`) are resolved against the branch list, longest match first.

## End-to-end test

`web/e2e/run.mjs` drives the real server in headless Chrome (system Chrome via puppeteer-core; pass
`--chrome PATH` or `CHROME_PATH` elsewhere). It signs up two fresh users through the UI and runs: sign
in/out, tokens, repo with README, tree/blob/raw, `git push` of several branches (one slashed) over smart
HTTP, branches, commits, commit diff, compare, PR create/files/commits/comment/review/approve, merge with
each method, issues (comment, labels, close/reopen), releases with asset upload + download, tags, stars,
forks, profile, search (repos/issues/code), SSH keys, empty-repo clone URLs, settings (default branch,
rename, delete), route-collision checks, dark mode, and 375 px mobile layout.

```sh
cd web && node e2e/run.mjs --base http://localhost:3109 --login e2e1 --password 'secret-pass-1' [--login2 e2e1-b] [--keep-going] [--headful]
```

Exit status is non-zero on any failure. Screenshots land in `web/e2e/screenshots/` (gitignored), with a
`FAIL-*.png` for a failed step. Every `git` it runs uses `GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_GLOBAL=/dev/null`.

## Conventions

- Every request sends `credentials: 'same-origin'` and `cache: 'no-store'` (the API sends GitHub's
  `Cache-Control: max-age=60`, which would otherwise show stale lists after a change). Every non-GET request also sends `X-Forge-CSRF: 1`.
- Errors show the GitHub `message`, plus `errors[].message` or `field code` when present.
- Lists use `per_page` and `page` and follow the `Link` header (`rel="next"`/`"prev"`/`"last"`).
- The UI decides permissions from `repo.permissions` (`push` and `admin`), which must be present when the
  caller is authenticated.

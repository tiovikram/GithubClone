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
src/pages/          one module per area (Home, Auth, Profile, NewRepo, Tokens, Code, Commits, Issues, Pulls, RepoSettings)
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

Refs that contain slashes (`feature/x`) are resolved against the branch list, longest match first.

## Conventions

- Every request sends `credentials: 'same-origin'`. Every non-GET request also sends `X-Forge-CSRF: 1`.
- Errors show the GitHub `message`, plus `errors[].message` or `field code` when present.
- Lists use `per_page` and `page` and follow the `Link` header (`rel="next"`/`"prev"`/`"last"`).
- The UI decides permissions from `repo.permissions` (`push` and `admin`), which must be present when the
  caller is authenticated.

# Web UI build-out

Goal: every feature the server implements is usable from the web UI, the way github.com (GHES)
presents it. The server already passes 1105/1105 GHES 3.22 REST operations. The UI only covered
code, issues, pull requests, releases, search, tokens and SSH keys.

## How the UI is extended

Areas live in `web/src/areas/<area>/` and register themselves in `index.tsx` (an `Area`, see
`web/src/areas/types.ts`). The core shell renders what areas register:

| Extension point | Rendered by | Notes |
|---|---|---|
| `topRoutes` | `App.tsx`, under the global layout | `<Route>` elements; static segments (`orgs/…`, `site-admin/…`) win over `:owner` |
| `repoRoutes` | `App.tsx`, under `/:owner/:repo` | e.g. `<Route path="actions/*" …/>` |
| `repoTabs` | `RepoLayout.tsx` | order: Code 0, Issues 10, Pull requests 20, Releases 80, Settings 1000 |
| `repoSettings` | `/:owner/:repo/settings/*` sidebar | General (core) is order 0 |
| `userSettings` | `/settings/*` sidebar | Access tokens 500 and SSH keys 510 are core (group "Access") |
| `orgSettings` | org settings shell (rendered by the **orgs** area) | context `OrgCtx` |
| `adminSections` | site admin shell (rendered by the **admin** area, `/site-admin/*`) | context `AdminCtx` |
| `headerNav`, `userMenu` | `Layout.tsx` | |
| `profileTabs`, `orgProfile` | `pages/Profile.tsx` | `orgProfile` replaces the profile page for organizations |
| `slots` | see `SlotProps` in `types.ts` | `header.right`, `repo.header.actions`, `repo.code.sidebar`, `pull.merge.top`, `pull.sidebar`, `issue.sidebar`, `commit.header`, `home.sidebar`, `profile.sidebar` |

Settings shells use `components/SettingsShell.tsx` (`SettingsShell`, `SettingsSidebar`,
`pickSection`); a section's `path` may contain slashes and the longest match wins, so a
section can own sub-pages (`secrets/actions/new`). Read the splat with `useParams()["*"]`.

## Areas and ownership

Each area owns its domain at every level (repo, org, user, site admin), through the registries.

| Area | Owns | Backend / Vite ports | DB |
|---|---|---|---|
| planning | Issues and PR extras: labels and milestones pages, assignees, issue dependencies/sub-issues, reactions, timeline events, review requests, line comments and suggestions on the Files tab, PR reviews UI; global `/issues` and `/pulls` dashboards; Projects (v2) at user/org/repo level; Discussions | 3410 / 5213 | forge_web_planning |
| repoadmin | Repo settings: general extras (features, merge buttons, archive, transfer, visibility, template), collaborators and teams, invitations (incl. the invitee's accept page), branches and branch protection, rulesets (repo/org/enterprise) with rule insights and bypass requests, tags, webhooks (repo and org) with deliveries/redeliver, deploy keys, autolinks, Pages, custom properties (repo/org/enterprise), LFS; Insights tab (contributors, commits, code frequency, punch card, traffic, forks, network/dependency graph link) | 3420 / 5223 | forge_web_repoadmin |
| actions | Actions tab (workflows, runs, run detail with jobs/steps/logs, re-run/cancel/approve, artifacts, dispatch form, caches), checks on PRs and commits, commit statuses, environments (protection rules, reviewers, branch policies, secrets/vars) and deployments, secrets and variables (repo/env/org), runners and runner groups (repo/org/enterprise), Actions permissions (repo/org/enterprise), OIDC settings | 3430 / 5233 | forge_web_actions |
| security | Security tab: overview, security policy, repository advisories (draft, publish, credits), Dependabot alerts (and dismissal requests), code scanning alerts and analyses, secret scanning alerts (push-protection bypasses, delegated bypass), dependency graph and SBOM export, dependency review on PRs; code security configurations and "Advanced Security" settings (repo/org/enterprise); Dependabot secrets; global advisories browser | 3440 / 5243 | forge_web_security |
| orgs | Organization profile, create organization, people (members, outside collaborators, pending invitations, security managers), teams (nested, members, repos, external groups), organization settings shell + sections: profile, member privileges / base permission, organization roles, repository roles, blocked users, personal access token policy and approvals, org audit log, org migrations; user "Organizations" settings | 3450 / 5253 | forge_web_orgs |
| packages | Packages (user/org/repo: list, package page with versions, delete/restore, settings and access), container and npm install instructions; GitHub Apps (developer settings: register, edit, keys, permissions, install, installations and their repos, org installations, suspend), OAuth apps and authorizations, user "Applications" settings; private registries (org) | 3460 / 5263 | forge_web_packages |
| social | Notifications inbox (bell in header, filters, mark read, subscriptions), watch/unwatch button, followers/following, stars and starred repos, activity feed on the dashboard, gists (create, edit, fork, star, comments, revisions), user settings: public profile, account (rename, delete, export/migrations), appearance, emails, GPG keys, SSH signing keys, social accounts, blocked users, notification settings; markdown and emoji where missing | 3470 / 5273 | forge_web_social |
| admin | Site admin (`/site-admin/*`, visible to site admins): overview/stats, users (create, suspend, promote, rename, delete, impersonation), organizations, repositories, announcements, maintenance mode, settings / manage-ghes, license, global webhooks, pre-receive environments and hooks (plus their org/repo settings sections), LDAP mappings, SCIM, enterprise audit log, live migrations, enterprise teams | 3480 / 5283 | forge_web_admin |

Files outside `web/src/areas/` are shared. Keep changes to them minimal and additive, and list
them in your report. **planning** owns `pages/Issues.tsx`, `pages/Pulls.tsx`,
`components/Conversation.tsx` and `components/Diff.tsx`; **social** owns `pages/Home.tsx`.
Nobody else edits those. `api.ts`: do not add to it; put your endpoint helpers in
`areas/<area>/api.ts`, using `request`, `requestPage` and `requestAll` from `../../api`. CSS
goes in `areas/<area>/styles.css` (imported by the area's `index.tsx`), using the tokens in
`src/styles.css` so light and dark themes both work. Areas do not import from each other.

## Running

```sh
scripts/web-area-server.sh <area> <backend-port> <vite-port>      # forge + vite; use the vite URL
cd web && npx tsc --noEmit                                         # must stay clean
cd web && node e2e/run.mjs --base http://localhost:<vite-port> --login <fresh> --password 'e2e-pass-123' \
  --areas <area> --only-areas                                      # your flows only
```

The first account on a fresh database is a site admin. If the server is missing something the
UI needs, or has a bug, fix it in `crates/forge` with a test (`CARGO_BUILD_JOBS=2`, your own
build: set `FORGE_BIN` to it), and keep REST responses GitHub-shaped.

## Definition of done (per area)

- Every capability in the area's row is reachable from the UI by clicking, not only by URL,
  and works end to end against the real server (create, view, edit, delete where they apply).
- Permissions are respected: controls a user can't use are hidden or disabled, and pages the
  user can't see show the 404 view.
- `e2e/areas/<area>.mjs` drives every page of the area in the browser, with screenshots, and
  passes on a fresh database and on a rerun. It includes a 375 px width check (no horizontal
  overflow) for the area's main pages, and no uncaught page errors.
- `docs/web/<area>.md`: a table from feature → UI location → e2e step, plus anything left out
  and why.
- Typecheck clean; the core e2e (`node e2e/run.mjs` without `--only-areas`) still passes.

## Machine rules

Use `caffeinate -dims` for long runs, `CARGO_BUILD_JOBS=2`, your own ports and DB only. Never
touch the user's git config, credentials or keychain, `~/.npmrc`, or the `polymarket-society`
container. Nothing that costs money or touches an account.

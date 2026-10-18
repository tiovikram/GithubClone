# Web UI: site administration (area `admin`)

Code: `web/src/areas/admin/` (pages, `api.ts`, `styles.css`), e2e `web/e2e/areas/admin.mjs`.
Server additions: `crates/forge/src/forge_admin.rs` (forge-only `/api/forge/...` endpoints) and a
maintenance-mode change in `crates/forge/src/enterprise/mod.rs`; tests in
`crates/forge/tests/enterprise.rs` (`web_admin_listings_details_and_password_reset`,
`maintenance_status_endpoint_and_spa_shell`).

Everything lives under **`/site-admin/*`**, reachable from the avatar menu → **Site admin**
(shown only to site admins). Non-admins get the 404 view; the API answers 403.

## Features

| Feature | UI location | API | e2e step (`admin: …`) |
|---|---|---|---|
| Shell, sidebar of every registered section, 404 for non-admins | `/site-admin`, user menu "Site admin" | `GET /api/forge/admin/info` | "user menu links to the site admin shell", "hidden from non-admins" |
| Overview: stats (users, orgs, repos, issues, PRs, milestones, comments, gists, hooks, pages), maintenance + license cards | Overview | `GET /enterprise/stats/all` | "user menu links…" |
| Users: search, filter (admins / suspended / active), list | Accounts → Users | `GET /api/forge/admin/users` | "search users and open a user" |
| User page: emails, password state, suspension + reason, LDAP DN, organizations, repositories, SSH keys (delete), access tokens (revoke) | Users → *login* | `GET /api/forge/admin/users/{login}`, `DELETE /admin/keys/{id}`, `DELETE /admin/tokens/{id}` | "search users…", "create user…" |
| Suspend (reason) / unsuspend | User page → Danger zone | `PUT/DELETE /users/{u}/suspended` | "suspend user with reason…", "unsuspend user…" |
| Promote / demote site admin | User page → Administrative actions | `PUT/DELETE /users/{u}/site_admin` | "promote and demote site admin" |
| Create user (optional password, optionally suspended) | Users → New user | `POST /admin/users` + `PUT /api/forge/admin/users/{u}/password` | "create user with password, rename, impersonate, delete" |
| Rename, delete user | User page | `PATCH/DELETE /admin/users/{u}` | same |
| Reset password (signs the user out) | User page | `PUT /api/forge/admin/users/{u}/password` | same |
| Impersonation OAuth token (scopes), revoke all | User page | `POST/DELETE /admin/users/{u}/authorizations` | same |
| LDAP user mapping + sync | User page, and Authentication → LDAP | `PATCH /admin/ldap/users/{u}/mapping`, `POST …/sync` | "LDAP mapping and sync from the user page", "LDAP page maps and syncs a team" |
| LDAP team mapping + sync (org + team slug) | Authentication → LDAP | `PATCH /admin/ldap/teams/{id}/mapping`, `POST …/sync` | "LDAP page maps and syncs a team" |
| Organizations: search, create with owner, rename, link to org settings | Accounts → Organizations | `GET /api/forge/admin/orgs`, `POST /admin/organizations`, `PATCH /admin/organizations/{org}` | "create organization with owner and rename it" |
| Repositories (incl. private): search, filter, archive/unarchive, delete, link to settings | Accounts → Repositories | `GET /api/forge/admin/repos`, `PATCH/DELETE /repos/{o}/{r}` | "repositories search, archive, unarchive, delete" |
| Enterprise teams: create, rename/description/org selection, members add/remove, org assign/unassign, delete | Accounts → Enterprise teams | `/enterprises/{e}/teams/…` | "enterprise team create, members, organization assignment, delete" |
| All SSH keys (user + deploy), delete | Accounts → SSH keys | `GET /admin/keys`, `DELETE /admin/keys/{ids}` | "SSH keys and access tokens listings" |
| All access tokens, revoke | Accounts → Access tokens | `GET /admin/tokens`, `DELETE /admin/tokens/{id}` | same |
| Announcement: set (Markdown), expiry, user-dismissible, remove; banner for every user | Messages → Announcement | `PATCH/DELETE /enterprise/announcement` | "announcement shows for other users, then clears" |
| Maintenance mode: enable now, schedule, disable, message, IP exception list | Management console → Maintenance mode | `GET/POST /manage/v1/maintenance`, `GET /api/forge/maintenance` | "maintenance mode on…", "schedule maintenance…" |
| Maintenance page for users (full page), admin reminder bar, scheduled notice | every page (slot `header.right`) | `GET /api/forge/maintenance` | same, plus 375px check |
| Management console settings (sign-up, private mode, repository invitations, hostname, auth mode, Pages, SMTP, raw JSON), save, save and apply, last run + events | Management console → Settings | `GET/PUT /manage/v1/config/settings`, `POST/GET /manage/v1/config/apply`, `GET …/apply/events` | "settings save and apply" |
| Nodes, cluster status, replication, versions, system requirements, upgrade status | Management console → Nodes and health | `/manage/v1/config/nodes`, `cluster/status`, `replication/status`, `version`, `checks/system-requirements`, `upgrade/status` | "nodes, license and SSH access pages" |
| License: seats, details, check, upload | Management console → License | `GET /enterprise/settings/license`, `GET/PUT /manage/v1/config/license`, `GET …/check` | same |
| Administrative SSH keys (add/remove) | Management console → Administrative SSH | `/manage/v1/access/ssh` | same |
| Global webhooks: create (URL, content type, secret, SSL, `user`/`organization` events, active), edit, ping, delete | Code and automation → Global webhooks | `/admin/hooks…` | "global webhook create, ping, edit, delete" |
| Pre-receive environments: add (download starts), status (auto-refresh while downloading), re-download, edit, delete | Code and automation → Pre-receive environments | `/admin/pre-receive-environments…` | "pre-receive environment add…, delete" |
| Pre-receive hooks: create/edit (script repo + path, environment, enforcement, allow downstream), delete; a push is rejected with the script's output | Code and automation → Pre-receive hooks | `/admin/pre-receive-hooks…` | "pre-receive hook rejects a push", "delete pre-receive hook (push accepted again)" |
| Repository-level enforcement override / reset | Repo settings → Code and automation → Pre-receive hooks (`/{o}/{r}/settings/pre-receive-hooks`) | `/repos/{o}/{r}/pre-receive-hooks/{id}` | "repository settings override pre-receive hook enforcement" |
| Organization-level enforcement + "allow repository admins" / reset | Org settings → Code and automation → Pre-receive hooks (`/organizations/{org}/settings/pre-receive-hooks`) | `/orgs/{org}/pre-receive-hooks/{id}` | "organization settings pre-receive hooks section" (SKIP line + API check while the orgs area's settings shell is not merged) |
| SCIM: provisioned users and groups, user and group pages | Authentication → SCIM | `/scim/v2/enterprises/{e}/Users`, `/Groups` | "SCIM lists provisioned users and groups" |
| Audit log: qualifier search, quick filters, paging, export JSON/CSV (up to 1000 events) | Compliance → Audit log | `GET /enterprises/{e}/audit-log` | "audit log shows the actions, filters and exports" |
| Live migrations: create, start/pause/resume/cancel/cutover/revert | Compliance → Live migrations | `/enterprise/live-migrations…` | "live migration create and start" |
| 375 px layout of the main pages and the maintenance page | — | — | "375px pages have no horizontal overflow" |

The e2e needs `--login` to be a site admin (first account on a fresh DB); otherwise its first
step fails with a message saying so. It turns maintenance mode off in `finally` blocks and
un-suspends / demotes the second user at the start in case an earlier run died midway.

## Server changes

- `GET /api/forge/maintenance` (public, exempt from maintenance mode): `{status, active, message,
  scheduled_time, bypass}` — lets the SPA show the maintenance page instead of failing requests.
- During maintenance, browser navigations (GET with `Accept: text/html`, not `/api/`, not git)
  and `/assets/*` now get the SPA shell (pages with status 503) instead of a JSON body, so users
  see the maintenance page in production builds too. API and git clients still get JSON 503.
- `/api/forge/admin/{info,users,users/{login},users/{login}/password,orgs,repos}`: GHES's REST
  API cannot list/search accounts with their admin state (suspension, reason, LDAP DN) — the GHES
  stafftools UI reads that internally. Site admins only; REST responses are unchanged.

## Left out

- Global webhook delivery log / redeliver: there is no global-hook deliveries endpoint (GHES has
  none either).
- Creating/editing SCIM identities in the UI: SCIM is written by the identity provider; the UI
  is read-only (the e2e provisions through the API).
- Upgrades (download/apply): forge is upgraded by deploying a new image; the Nodes page shows
  the server's explanation.
- "Dormant users" filter and user emails management: forge does not track last activity per
  user; emails are shown, users manage them in their own settings.
- Enterprise-level Actions, code security, rulesets and custom properties pages: owned by the
  actions, security and repoadmin areas, which add them as `adminSections`.

## For other areas: the /site-admin shell and `AdminCtx`

Register pages with `adminSections` in your `Area`:

```tsx
adminSections: [{ key: "runners", path: "actions/runners", label: "Runners", group: "Code and automation", order: 430, element: Runners }],
```

- `element` receives `{ ctx }` with `AdminCtx = { user, enterprise }`; use `ctx.enterprise` for
  `/enterprises/{enterprise}/…` endpoints. The shell already guarantees the viewer is a site admin.
- Groups and orders used here: (ungrouped) Overview 0 · **Accounts** 100–150 · **Messages** 200 ·
  **Management console** 300–340 · **Code and automation** 400–420 (actions: 430–490) ·
  **Authentication** 500–510 · **Compliance** 600–610 · suggested **Security** 700+ ·
  **Policies** (rulesets, custom properties) 800+. A new group name appears after the existing ones.
- Sub-pages: a section owns every URL below its `path` (longest match wins). Read the rest with
  `useParams()["*"]`, which is the full path after `/site-admin/` (e.g. `actions/runners/12`).
  On a two-segment URL (`/site-admin/<path>`, served by the `site-admin/:section` route) it is
  `undefined`: treat that as the section root.
- Why the extra route: react-router ranks `site-admin/*` below `/:owner/:repo`'s index route for
  two-segment URLs, so `/site-admin/users` would render as a repository. The same applies to any
  `topRoutes` of the form `<static>/*` (e.g. `/orgs/x`): add an explicit two-segment route.
- Link to pages with `/site-admin/<path>`; style with the shared tokens (see `styles.css` here).

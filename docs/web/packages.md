# Web UI: packages area

Packages (user, organization, repository), GitHub Apps, OAuth apps and their authorizations,
the device-flow page and organization private registries. Code: `web/src/areas/packages/`;
e2e: `web/e2e/areas/packages.mjs`; server endpoints added for the UI:
`crates/forge/src/api/packages_forge.rs`, `crates/forge/src/api/apps_manage.rs` (tests:
`crates/forge/tests/web_packages.rs`).

Run: `scripts/web-area-server.sh packages 3460 5263`, then
`node e2e/run.mjs --base http://localhost:5263 --login <fresh> --password 'e2e-pass-123' --areas packages --only-areas`.
The registries (`/v2/`, `/npm/`) and the device-flow endpoints (`/login/device/code`,
`/login/oauth/access_token`) are not proxied by Vite, so the e2e calls them on the backend
(`--backend URL`, else `FORGE_BACKEND`, else the area port scheme 52x3 → 34x0). The UI shows
install commands for the browser's host (`location.host`), which is right in production where
forge serves the SPA and the registries on one host.

## Features

| Feature | UI location | e2e step |
|---|---|---|
| User packages (all types, type filter, search) | Profile → **Packages** tab (`/{user}?tab=packages`; `/users/{user}/packages` redirects) | "profile Packages tab lists and filters packages" |
| Organization packages | `/orgs/{org}/packages` (the org profile's Packages tab links here) | "organization packages page" |
| Repository packages | Code page sidebar box **Packages**; `/{owner}/{repo}/packages` | "repository sidebar and repository Packages page" |
| Package page: README (npm, from the tarball), description (container label), install commands (`npm install`, `.npmrc` line, `docker pull HOST/owner/name:tag`), recent versions, details sidebar | `/{users\|orgs}/{owner}/packages/{type}/package/{name}` (GitHub's URL scheme) | "npm package page …", "container page …" |
| Versions list (tags, digests), deleted versions | `…/packages/{type}/{name}/versions` (`?state=deleted`, admins) | "delete and restore a version" |
| Version page (digest, tags, size, media type, publisher, config digest / integrity, README, install command for that version) | `…/packages/{type}/{name}/{version_id}` | "npm package page …", "container page …" |
| Delete / restore a version | Versions list buttons; version page | "delete and restore a version" |
| Package settings: linked repository, inherit access, visibility (public / private / internal for orgs), delete package | `…/packages/{type}/{name}/settings` (package admins) | "settings link a repository, change visibility, inherit access"; "delete a package and restore it" |
| Manage access (per-user read / write / admin) | Package settings → Manage access | "manage access grants a second user read on a private image" |
| Restore deleted packages | Settings → **Packages** (`/settings/packages`); org settings → Packages | "delete a package and restore it from Settings → Packages"; org step |
| Register a GitHub App (name, description, homepage, callback URLs, setup URL, webhook URL/active, permissions, events, installable by any account or only this one) | Settings → Developer settings → **GitHub Apps** → New GitHub App (`/settings/apps/new`; orgs: `/organizations/{org}/settings/apps/new`) | "register a GitHub App in developer settings" |
| Private key at registration and on demand: browser download of the PEM plus a one-time view; key list with fingerprints; delete key (not the last one) | App settings → General (`/settings/apps/{slug}`) | "register …" (checks the .pem download), "edit the app, rotate keys and client secret" |
| Client secret (regenerate, shown once), webhook secret, edit basic info / URLs / webhook | App settings → General | "edit the app, rotate keys and client secret" |
| Permissions & events | App settings → Permissions & events (`/settings/apps/{slug}/permissions`) | "permission update needs the installation owner's approval" |
| Make public / private, delete app | App settings → Advanced | "uninstall, make public, delete the app" |
| Install: from the app's Install App tab or its public page; choose account (you, orgs you own), all / selected repositories | `/settings/apps/{slug}/installations`, `/apps/{slug}`, `/apps/{slug}/installations/new[/permissions?target_id=]` | "install on selected repositories from the Install App tab"; "… public app page" |
| Installation token scope (JWT signed in node, `ghs_` token confined to the selected repos and permissions; issue created as the bot) | — (API) | "installation token (JWT signed in node) is scoped to the installation" |
| Installation settings: repository access, accept updated permissions, suspend / unsuspend, uninstall | Settings → **Applications** → Installed GitHub Apps → Configure (`/settings/installations/{id}`); org: `/organizations/{org}/settings/installations/{id}` | "permission update …", "suspend, unsuspend and change repository access", "uninstall …" |
| Installed GitHub Apps (user) | `/settings/installations` | "Settings → Applications lists the installation" |
| Organization installations | Org settings → Third-party Access → **GitHub Apps** (`/organizations/{org}/settings/installations`) | org settings step |
| Authorized GitHub Apps (user-to-server grants), revoke | `/settings/installations/authorized` | "device flow for the GitHub App → Authorized GitHub Apps → revoke" |
| OAuth apps: list, register, edit, client secret, delete (user and org) | Developer settings → **OAuth Apps** (`/settings/applications`, `/settings/applications/new`, `/settings/applications/{slug}`) | "oauth: register, edit and rotate …", "oauth: delete the OAuth app" |
| Authorized OAuth Apps, revoke | `/settings/installations/oauth` | "oauth: device flow approval, Authorized OAuth Apps, revoke" |
| Device-flow verification page (enter code, see app and scopes, authorize / cancel) | `/login/device` (the server's `verification_uri`) | both device-flow steps (also an invalid code) |
| Private registries: list, create (token / username+password secrets sealed in the browser with libsodium `crypto_box_seal` against the org key; OIDC types with their fields), edit, delete, repository access | Org settings → Code, planning, and automation → **Private registries** (`/organizations/{org}/settings/private-registries`) | org settings step |
| 375 px layout | profile packages, package page, versions, package settings, org packages, new app, applications, device page, repo packages | "main pages have no horizontal overflow at 375px" |

The org settings sections render inside the orgs area's shell (`/organizations/:org/settings/*`).
The e2e step gates on `.settings-nav` and logs `SKIP` when the shell is missing (on branches
without the orgs area).

## Server changes for the UI

* `GET /api/forge/packages/{owner}` (every type, `package_type`, `q`), `…/deleted`,
  `GET /api/forge/repos/{o}/{r}/packages`, `GET|PATCH /api/forge/packages/{owner}/{type}/{name}`
  (details incl. README / settings), `…/access[/{login}]`, `…/versions/{id}`.
* Granular package access: table `package_access` (migration 1595), honored by
  `packages::package_perm` (REST, `/v2/`, `/npm/`).
* `GET|PATCH /api/forge/apps/{slug}`, `GET /api/forge/apps/{slug}/keys`,
  `DELETE /api/forge/apps/{slug}/keys/{id}`, `POST /api/forge/apps/{slug}/client-secret`;
  `GET /api/forge/apps` returns slug and owner for OAuth apps too.
* `GET|PATCH|DELETE /api/forge/installations/{id}`, `PUT|DELETE …/suspended`,
  `POST …/permissions` (accept the app's updated permissions; app edits don't change existing
  installations until then, like GitHub).
* `GET /api/forge/authorizations?kind=app|oauth`, `DELETE /api/forge/authorizations/{grant_id}`.
* `GET /api/forge/device/{code}` also returns the requesting app (name, kind, owner).

## Left out

* **Multiple client secrets per app**: forge stores one client secret per app; "Generate a new
  client secret" replaces it (no list / delete of individual secrets).
* **Renaming an app keeps its slug** (and bot login), unlike GitHub, so URLs and the bot user
  don't move.
* **Team grants in package access**: only users can be granted roles; GitHub also allows teams.
* **Package download counts and per-version "Installation" stats**: forge doesn't record downloads.
* **OAuth web flow (`/login/oauth/authorize`)**: the server implements only the device flow, so
  OAuth apps are authorized through `/login/device`.
* **App manifest flow page** (`/settings/apps/new?manifest=…`): the server's manifest endpoints
  exist, but there is no UI to post a manifest; apps are registered with the form.
* **Transfer app ownership**, **app logo upload**, **installation requests** (non-owner members
  asking an owner to install): not implemented by the server.
* **Repository settings → GitHub Apps** list: installed apps are managed from the account's
  settings (user / org), not per repository.

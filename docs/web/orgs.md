# Web UI: organizations (area `orgs`)

Code: `web/src/areas/orgs/` (`index.tsx` registers everything; `api.ts` endpoint helpers;
`common.tsx` org loader + header/tabs; `OrgProfile.tsx`, `NewOrg.tsx`, `People.tsx`, `Teams.tsx`,
`Settings.tsx`, `UserOrgs.tsx`; `styles.css`). E2E: `web/e2e/areas/orgs.mjs` (steps prefixed
`orgs:`). Server additions: `crates/forge/src/api/orgs_invitations.rs`, migration
`1556_org_invitations.sql`, tests `crates/forge/tests/orgs_invitations.rs`.

## Features

| Feature | UI location | e2e step |
|---|---|---|
| Create organization | User menu → "New organization", `/settings/organizations` → "New organization", `/organizations/new` (`POST /api/forge/orgs`, then `PATCH /orgs/:org` for the contact email) | `create organization from the user menu` |
| Organization profile (header with avatar, name, description, location, URL, email; tabs Overview, Repositories, Projects, Packages, People, Teams, Settings) | `/:org` (registered as `orgProfile`, replaces the user profile) | `create organization…`, `profile tabs, overview and repository filters` |
| Overview: popular repositories (forge has no pinning, so the top repositories by stars/recency, as github.com shows "Popular repositories" without pins), recent repositories, people avatars, top languages | `/:org` | `profile tabs…` |
| Repositories tab: search, type (all/public/private/sources/forks/archived), sort (updated/name/stars), pagination | `/orgs/:org/repositories` | `profile tabs…` |
| New repository in the organization | Repositories tab / Overview → "New repository", `/organizations/:org/repositories/new` (`POST /orgs/:org/repos`) | `create repositories in the organization (UI)` |
| Projects and Packages tabs | links to `/orgs/:org/projects` (planning area) and `/orgs/:org/packages` (packages area) | `profile tabs…` (hrefs) |
| People: members with role badges, search, role filter | `/orgs/:org/people` | `change role to owner and back` |
| Invite member by username or email, role (member/owner), teams | People → "Invite member" (`POST /orgs/:org/invitations`) | `invite the second user (UI)`, `convert to outside collaborator, remove, re-invite` |
| Pending invitations (cancel) | People → Invitations, `/orgs/:org/people/pending_invitations` | `invite the second user (UI)` |
| Failed invitations (expired after 7 days, or invitee blocked) | People → Failed invitations, `/orgs/:org/people/failed_invitations` | `blocked users fail their invitation` |
| Change role owner ↔ member | People → role select | `change role to owner and back` |
| Remove member / convert to outside collaborator | People → row buttons | `convert to outside collaborator…` |
| Outside collaborators (list, remove from all repositories) | People → Outside collaborators, `/orgs/:org/outside-collaborators` | `convert to outside collaborator…` |
| Security managers (add/remove teams) | People → Security managers, `/orgs/:org/people/security_managers` | `security managers` |
| Teams list (nested tree, search), new team | `/orgs/:org/teams`, `/orgs/:org/new-team` | `create a team and a child team` |
| Team page: members (add, role member/maintainer, remove; pending invitations with the team), child teams ("Add a team"), repositories (add with permission incl. custom repository roles, change, remove) | `/orgs/:org/teams/:slug`, `…/teams`, `…/repositories` | `create a team…`, `team with write access allows push`, `repository roles CRUD` |
| Team settings: name, description, parent, visibility, notifications; external groups (connect/remove an IdP group when groups exist); delete team | `/orgs/:org/teams/:slug/settings` | `team settings (rename, notifications) and delete child team` |
| Organization settings shell (owners only, 404 otherwise; hosts other areas' sections) | `/organizations/:org/settings/*` | `settings shell + General profile`, `invitee accepts…` (member gets 404) |
| General: display name, public email, description, URL, location, social account, billing email; rename; delete | Settings → General (`profile`) | `settings shell + General profile`, `rename the organization`, `delete the organization` |
| Member privileges: base permission (none/read/write/admin), repository creation (public/private), private forking, Pages, team creation, web commit signoff, deploy keys, projects | Settings → Member privileges (`member_privileges`) | `base permission none hides…`, `base permission read allows clone, not push`, `member privileges (repository creation)` |
| Organization roles: list predefined + custom, create/delete custom roles with fine-grained permissions, assign/revoke to members and teams | Settings → Organization roles (`organization_roles`) | `organization roles (custom role, assign, effect, revoke)` |
| Repository roles: custom repository roles create/edit/delete | Settings → Repository roles (`repository_roles`) | `repository roles CRUD` |
| Blocked users: block/unblock (blocks fail pending invitations and stop issues, PRs and comments in org repos) | Settings → Blocked users (`blocked_users`) | `blocked users fail their invitation` |
| Personal access tokens: pending requests (approve/deny), active fine-grained tokens (revoke) | Settings → Personal access tokens | `personal access tokens page` |
| Audit log with phrase filter and paging | Settings → Audit log (`audit-log`) | `audit log` |
| Export (org migrations): start with repositories/lock/exclude attachments, state, download, delete archive | Settings → Export (`migrations`) | `export (migration) and download` |
| User settings "Organizations": your orgs with role, settings link, publicize/conceal membership, leave; pending invitations accept/decline | `/settings/organizations` (user settings section, group Access) | `invitee accepts…`, `accept in settings, publicize…`, `leave the organization` |
| Invitation page: join or decline | `/orgs/:org/invitation` (linked from settings, the org header and the dashboard) | `invitee accepts from Organizations settings → invitation page` |
| Dashboard notice of pending org invitations | `home.sidebar` slot | `accept in settings…` (best effort: Home is owned by social) |
| Mobile layout | all main pages at 375 px | `375px no horizontal overflow` |

## Server changes

- **Organization invitations** (api.github.com shape; GHES 3.22 has no invitation endpoints and its
  `PUT /orgs/{org}/memberships/{username}` keeps adding members directly, so the REST conformance
  suite is unaffected): `GET/POST /orgs/{org}/invitations`, `DELETE /orgs/{org}/invitations/{id}`,
  `GET /orgs/{org}/invitations/{id}/teams`, `GET /orgs/{org}/failed_invitations`,
  `GET /orgs/{org}/teams/{team_slug}/invitations`. The invitee sees a `pending` membership in
  `GET /user/memberships/orgs[?state=pending]` and `GET /user/memberships/orgs/{org}` (owners see it
  in `GET /orgs/{org}/memberships/{username}`), accepts with `PATCH /user/memberships/orgs/{org}`
  `{"state":"active"}` (joins with the role and teams of the invitation) and declines with the
  forge-only `DELETE /api/forge/user/memberships/orgs/{org}`.
- **Organization blocks**: `GET /orgs/{org}/blocks`, `GET/PUT/DELETE /orgs/{org}/blocks/{username}`.
  Members can't be blocked (422); blocked users can't be invited or added, their pending
  invitations fail, and they get 403 creating issues, pull requests or comments in the org's
  repositories.
- **Owner rename**: forge-only `PATCH /api/forge/orgs/{org}` `{"login"}` (GHES only has the
  site-admin `PATCH /admin/organizations/{org}`).
- **`GET /orgs/{org}/members`**: supports `role` (`all`/`admin`/`member`) and `filter`, and like
  GitHub only returns concealed members to members (others get public members).

Regression tests: `cargo test -p forge --test orgs_invitations`.

## For other areas: the org settings shell and `OrgCtx`

The orgs area renders `/organizations/:org/settings/*` with `SettingsShell` (container) below the
org header, for organization owners and site admins only (everyone else gets the 404 view). Every
section registered in an area's `orgSettings` appears in the sidebar; the
first visible section (General, order 0) is shown at the root.

```tsx
orgSettings: [
  { key: "secrets", path: "secrets/actions", label: "Actions secrets", group: "Security", order: 410, element: OrgSecrets },
]
// element: ({ ctx }: { ctx: OrgCtx }) => …
// ctx.org     the organization (GET /orgs/:org as the owner: includes owner-only fields)
// ctx.login   its login (use for API paths)
// ctx.isOwner always true inside this shell (kept for sections that hide owner-only controls)
// ctx.reload  re-fetches the org (call after changing it)
```

Paths may contain slashes and the longest match wins (`useParams()["*"]` is the splat, e.g.
`secrets/actions/new`). Groups appear in first-seen order of the sorted sections; the orgs area
uses: (none) General 0; **Access** Organization roles 100, Repository roles 110, Member privileges
120; **Moderation** Blocked users 300; **Third-party Access** Personal access tokens 600;
**Archive** Audit log 900, Export 910. Suggested slots for others: "Code, planning, and
automation" 200–290 (webhooks, rulesets, custom properties, Actions, runners, Packages),
"Security" 400–490 (code security, secrets), "Third-party Access" 600–690 (GitHub Apps, OAuth app
policy, private registries). Link to a section with `/organizations/${login}/settings/${path}`.

The org header (`OrgHeader` in `common.tsx`) links the Projects tab to `/orgs/:org/projects` and
the Packages tab to `/orgs/:org/packages`; the areas that own those register the routes.

## Left out

- **Pinned repositories**: no REST endpoint for profile pins; the overview shows popular repositories instead.
- **Fine-grained PAT policy toggles**: forge issues classic tokens only, and the REST API has no
  policy endpoint; the page lists (empty) requests and grants with approve/deny/revoke.
- **Billing managers / billing, 2FA requirement, SAML/SSO, verified domains, team discussions,
  team code review settings, org avatar upload**: not implemented by the server.
- **External groups provisioning**: groups come from enterprise SCIM (admin area); the team page
  only connects/disconnects existing groups.
- **Failed invitation by expiry** is computed from the invitation's age (7 days); e2e covers the
  "blocked" failure only (expiry would need clock control).

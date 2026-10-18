# Web UI: social area

Code: `web/src/areas/social/` (registered in `index.tsx`), plus `web/src/pages/Home.tsx` (owned by
this area). E2E: `web/e2e/areas/social.mjs` (step names below are prefixed `social:`).
Server additions: `crates/forge/src/api/social_forge.rs`, migration `1575_social.sql`, tests in
`crates/forge/tests/social.rs`.

## Features

| Feature | UI location | E2E step |
|---|---|---|
| Notifications bell with unread dot (polls every 60 s and on navigation) | header, slot `header.right` | other user opens an issue → bell dot + inbox |
| Notifications inbox: Inbox (unread) / All / Participating, reason filters, repository filter, pagination | `/notifications` (`?filter=all\|participating`, `?reason=`, `?repo=o/r`) | other user opens an issue → bell dot + inbox |
| Mark as read, Done, Mark all as read (per repo when filtered) | `/notifications` row actions and toolbar | same step |
| Unsubscribe from a thread (mutes it and marks it done) | `/notifications` row action | unsubscribe from a thread in the inbox |
| Watch button: Participating and @mentions / All Activity / Ignore / Custom (issues, pull requests, releases), watcher counter | repository header, slot `repo.header.actions` | repo watch button |
| Watchers page (linked from the watch counter) | `/{o}/{r}/watchers` | repo watch button |
| Stargazers page (linked from the star counter) | `/{o}/{r}/stargazers` | star → stargazers page |
| Watched repositories, unwatch | `/watching` (user menu "Watched repositories", inbox sidebar) | /watching lists and unwatches |
| Issue / PR Subscribe / Unsubscribe with the reason ("because you authored…", "watching…") | issue sidebar (`issue.sidebar`), PR sidebar (`pull.sidebar`) | issue sidebar Subscribe / Unsubscribe; pull request sidebar subscription |
| Follow / Unfollow, follower and following counts | profile sidebar (`profile.sidebar`) | follow from profile |
| Followers / Following lists | profile tabs `?tab=followers`, `?tab=following` | follow from profile |
| Public activity on profiles | profile tab `?tab=activity` | activity feed on the dashboard and profile |
| Dashboard activity feed: "Following" (received events: followed users, watched and starred repos) and "Your activity" | `/` (pages/Home.tsx, below "Recent issues and pull requests") | activity feed on the dashboard and profile |
| Gists: your gists, starred, discover (public), user gists (others see public only) | `/gist/{login}`, `/gists/starred`, `/gists/discover`; user menu "Your gists"; profile sidebar "Gists" | gists — create; gists — public gist on discover |
| Create gist with multiple files, secret or public | `/gists/new` | gists — create (multiple files) via the user menu |
| View gist: syntax highlighting, Markdown preview, Raw links, Share / Clone / Embed box | `/gist/{login}/{id}` | gists — create |
| Edit gist (rename, change, add, remove files) | `/gist/{login}/{id}/edit` | gists — edit |
| Revisions with per-file line diffs, view a file at a revision | `/gist/{login}/{id}/revisions`, `/gist/{login}/{id}/{sha}` | gists — edit … revisions |
| Star / unstar, fork, forks list | gist header buttons, `/gist/{login}/{id}/forks` | gists — star, fork, comments |
| Gist comments with Write/Preview, edit, delete | gist page | gists — star, fork, comments |
| Delete gist | gist header | gists — public gist on discover, then delete |
| Public profile: name, public email, bio, URL, Twitter, company, location, "Available for hire"; social accounts (up to 4) | `/settings/profile` (also `/settings`), shown on the profile sidebar | settings — public profile |
| Account: change username, export account data (user migration archive), delete account | `/settings/admin` | account — rename; account — export data; account — delete a throwaway account |
| Appearance: theme (system / light / dark), kept in sync with the header theme toggle (`localStorage["forge-theme"]`) | `/settings/appearance` | settings — appearance |
| Emails: add, remove, make primary | `/settings/emails` | settings — emails |
| Password change | `/settings/security` ("Password and authentication") | settings — change password |
| GPG keys and SSH signing keys: add, list, delete | `/settings/gpg_keys` (group "Access", order 515) | settings — GPG key and SSH signing key |
| Blocked users: block/unblock from settings or the profile sidebar; blocked users can't follow you, open issues/PRs or comment in your repositories | `/settings/blocked_users`, profile sidebar "Block user" | block user (profile + settings) |
| Explore: most starred / recently updated public repositories, people search | `/explore` (header "Explore") | explore |
| Markdown Write/Preview tabs | gist comments | gists — star, fork, comments |
| 375 px layout | all main pages above | pages at 375px have no horizontal overflow |

## Server changes

- `GET|PUT|DELETE /api/v3/user/blocks[/{username}]` (github.com shape). Blocking removes follows
  both ways; a blocked user gets 403 on follow, issue/PR creation and issue comments in the
  blocker's repositories.
- `GET|PUT /api/forge/repos/{o}/{r}/subscription` — watch state including "custom"
  (`watches.custom_events`); notification fan-out honours the selected kinds. The REST
  `PUT /repos/{o}/{r}/subscription` resets custom kinds.
- `GET|PUT|DELETE /api/forge/repos/{o}/{r}/issues/{n}/subscription` — subscribe to an issue or PR
  without an existing notification thread (GitHub's sidebar button), with the effective reason.
- `PUT /api/forge/user/password`, `PATCH /api/forge/user/login`, `DELETE /api/forge/user`
  (login + password confirmation; refused for site admins and sole organization owners).
- `PATCH /user` accepts `hireable`; `GET /user` and `GET /users/{u}` return `hireable`,
  `public_gists` (and `/user`: real `followers`/`following`/`private_gists`). Changing the primary
  email keeps the old one as a secondary address.
- Creating a repository records a `CreateEvent` (`ref_type: repository`), as GitHub does.

## Left out

- **Notification settings page** (email delivery, default watching): the server has no
  notification preferences or email delivery, so there is nothing to configure.
- **Email visibility ("Keep my email address private")**: GHES has no visibility endpoint and
  forge has no per-address visibility; the public email is chosen in the profile settings.
- **Pronouns**: not a field in the GHES user schema or the forge database.
- **Saved / "Done" inbox views**: the REST API can't list done or saved threads.
- **Emoji shortcodes**: rendering `:smile:` would need changes to the shared Markdown renderer
  (core `markdown.tsx`); left for the core owner.
- **Embed script (`.js`)**: the server has no gist embed script; the share box offers an iframe of
  the raw file instead.
- **Two-factor authentication, sessions list**: not implemented by the server.

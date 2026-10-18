# Web UI area: repoadmin

Repository settings (beyond the core General form), rulesets at repository / organization /
enterprise level with rule insights and push bypass requests, webhooks, invitations, Pages,
custom properties, and the repository **Insights** tab. Code: `web/src/areas/repoadmin/`;
e2e: `web/e2e/areas/repoadmin.mjs` (step names below are prefixed `repoadmin:`).

## Features

| Feature | UI location | e2e step |
|---|---|---|
| Settings sidebar groups (Access / Code and automation / Security / Integrations) | `/{o}/{r}/settings` | settings sidebar groups |
| Template repository, features (wikis, issues, discussions, projects, allow forking for private org repos), merge buttons + default commit messages, update-branch suggestion, auto-merge, delete head branches, web commit sign-off, Git LFS | Settings → General (below the core form, slot `repo.settings.general`), autosaved | general features, merge buttons, template, LFS |
| "Use this template" (generate a repository from a template) | header button on template repos → `/{o}/{r}/generate` | use this template |
| Change visibility, transfer ownership, archive / unarchive (+ "Public archive" badge) | Settings → General → Danger Zone (slot `repo.settings.danger`) | danger zone — change visibility / archive … / transfer … |
| Collaborators: add (with role, incl. custom repository roles), change role, remove; pending invitations: change role, cancel | Settings → Collaborators and teams (`settings/access`) | add collaborator …, change collaborator role …, invitations list … |
| Teams on org repositories: add, change role (incl. custom roles), remove | Settings → Collaborators and teams | org repository — teams and custom repository roles |
| Invitee side: dashboard "Pending invitations" box, `/invitations` list (accept / decline), `/{o}/{r}/invitations` accept page; user-menu link "Repository invitations" | dashboard sidebar slot, `/invitations`, `/{o}/{r}/invitations` | add collaborator (invitation) …, invitations list — decline … |
| Branches: default branch, classic branch protection rules list / full editor (required reviews + approvals, stale dismissal, code owners, last-push approval, dismissal restrictions, PR bypass allowances, status checks + strict, conversation resolution, signed commits, linear history, lock branch + fork syncing, enforce admins, push restrictions users/teams/apps + block creations, force pushes, deletions) / delete; enforcement checked with a real `git push` | Settings → Branches (`settings/branches`, `/new`, `/edit/<branch>`) | classic branch protection — create, enforce on push, edit, delete |
| Rulesets: list (incl. inherited org / enterprise rulesets), new branch / tag / push ruleset, every GHES 3.22 rule type with its parameters, ref targets (default branch, all, include / exclude patterns), bypass list (repository roles incl. custom, org admin, enterprise owner, deploy keys, teams, apps) with modes, enforcement (active / evaluate / disabled), edit, history, import / export JSON, delete; enforcement checked with `git push` | Settings → Rules (`settings/rules`, `/new?target=…`, `/<id>`, `/<id>/history`) | branch ruleset — create …; edit ruleset (evaluate), history, export; tag ruleset; delete |
| Tags (tag rulesets) | Settings → Tags (`settings/tags`) | edit ruleset … tag ruleset … |
| Rule insights (rule suites, filters, detail with each rule evaluation) | Settings → Rules → Insights (`settings/rules/insights[/<id>]`) | rule insights list and suite detail |
| Push bypass requests: list, detail, approve / deny with comment, cancel; requester's view | Settings → Rules → Bypass requests; `/{o}/{r}/exemptions[/<n>]` | push ruleset — blocked push creates a bypass request, admin approves, push succeeds |
| Webhooks: list with last-delivery status, add / edit (URL, content type, secret, SSL, push / everything / individual events, active), ping, test push, delete; Recent Deliveries with request headers + payload and response headers + body, redeliver | Settings → Webhooks (`settings/hooks`, `/new`, `/<id>`, `/<id>/deliveries`) | webhooks — create, ping, deliveries …, redeliver, edit, delete (a local HTTP receiver checks real delivery) |
| Deploy keys (read-only / write) | Settings → Deploy keys (`settings/keys`) | deploy keys add and delete |
| Autolink references (alphanumeric / numeric, preview) | Settings → Autolink references (`settings/key_links`) | autolink references add and delete |
| GitHub Pages: source (branch + folder, or Actions), live URL, visit, request build, recent builds, custom domain, enforce HTTPS, unpublish | Settings → Pages (`settings/pages`) | GitHub Pages — publish from branch, build, visit, custom domain, unpublish |
| Custom property values of a repository | Settings → Custom properties (org repos) | custom property values on an organization repository |
| Org: rulesets (+ repository targeting by name / property / id), rule insights, bypass requests, webhooks with deliveries, custom property definitions + per-repository values | org settings shell (orgs area): `/organizations/{org}/settings/rules`, `/hooks`, `/custom-properties` | organization settings — rulesets, webhooks, custom properties |
| Enterprise: rulesets (organization targeting), custom property definitions, push bypass requests | site admin shell (admin area): `/site-admin/rulesets`, `/custom-properties`, `/bypass-requests` | site admin — enterprise rulesets … (runs when the e2e user is a site admin, i.e. the first account on the DB; otherwise logs SKIP) |
| Insights tab: Pulse (period, merged / opened PRs, closed / new issues, commit summary, weekly participation), Contributors (per-author weekly charts), Commits (year + per-day), Code frequency, Punch card, Traffic (clones, visitors, referring sites, popular content), Community standards checklist, Forks (sorting), Dependency graph link (`/{o}/{r}/network/dependencies`, page built by the security area) | repo tab "Insights" → `/{o}/{r}/pulse`, `/graphs/*`, `/community`, `/forks` | insights — pulse, contributors, commits, code frequency, punch card; insights — traffic (views beacon + clones), community, forks |
| 375 px layout of the main pages | — | 375px pages have no horizontal overflow |

## Server changes (crates/forge)

- `GET /repos/{o}/{r}/traffic/views|clones|popular/referrers|popular/paths` and
  `GET /repos/{o}/{r}/community/profile` (api.github.com shapes; GHES lacks them). Views are
  reported by the web UI (`POST /api/forge/repos/{o}/{r}/traffic/view`, sent from a
  `repo.header.actions` slot component on every repository page); clones are counted at the
  upload-pack advertisement (HTTP) and upload-pack start (SSH). `api/repo_traffic.rs`.
- Enterprise rulesets: `/enterprises/{e}/rulesets[/{id}]` (site admins), applied to org-owned
  repositories selected by `organization_name` + repository conditions, enforced like org
  rulesets; the existing enterprise ruleset history endpoints now return versions.
- Collaborators and invitations accept custom repository role names on org repositories;
  `role_name` reports the custom role (drops the built-in-only check constraint).
- `PATCH /repos` stores `merge_commit_title/message`, `squash_merge_commit_title/message`,
  `allow_update_branch`, `use_squash_pr_title_as_default`, `web_commit_signoff_required`
  (validated like GitHub); PR merges without an explicit message use them. `GET /repos` reports
  `template_repository` for generated repositories.
- Forge-only helpers: `GET /api/forge/repos/{o}/{r}/lfs` (the LFS flag has no REST GET),
  `GET /api/forge/enterprise` (enterprise slug).
- Migration `1592_repoadmin.sql`; tests `crates/forge/tests/repoadmin_web.rs`.

## Left out

- Social preview image upload: the server has no storage / endpoint for it (not in GHES REST).
- "Internal" visibility: forge repositories are only public or private.
- Classic branch protection for wildcard patterns: the REST API (and forge) protect one named
  branch; patterns are covered by rulesets (the editor links there).
- Enterprise rule insights: GHES has no enterprise rule-suite endpoint (repo / org insights work).
- Wikis: the toggle is stored, but forge has no wiki pages to show.
- GitHub Pages renders `index.html` files only (no Jekyll / README fallback).

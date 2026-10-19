# Web UI: planning area

Issues and pull request extras, labels, milestones, the global issue / pull request
dashboards, Projects (v2) and Discussions. Code: `web/src/areas/planning/` plus the planning-owned
`pages/Issues.tsx`, `pages/Pulls.tsx`, `components/Conversation.tsx`, `components/Diff.tsx`.
E2E: `web/e2e/areas/planning.mjs` (steps are prefixed `planning:`).

## Features

| Feature | UI location | API | e2e step |
|---|---|---|---|
| Labels: list, search, sort, create (name, description, color picker / random color), edit, delete | `/{o}/{r}/labels` (Labels button on Issues / Pull requests lists, "Edit labels" in the label picker) | REST labels | labels page — create, edit, delete |
| Milestones: list (open/closed, sort, progress, % complete), create, edit, close/reopen, delete | `/{o}/{r}/milestones`, `/milestones/new`, `/milestones/{n}/edit` | REST milestones | milestones — create, edit, close, reopen |
| Milestone page (its issues and PRs, progress) | `/{o}/{r}/milestone/{n}` | REST issues `?milestone=` | issue list filters, search and milestone page |
| Issue templates (chooser, prefilled title/body) | New issue → `/{o}/{r}/issues/new/choose` → `/issues/new?template=` | GraphQL `issueTemplates` (server: Markdown templates in `.github/ISSUE_TEMPLATE`) | new issue from a template… |
| Labels / assignees / milestone when opening an issue | New issue sidebar (push access) | REST create issue | new issue from a template with labels, assignee and milestone |
| Issue list filters: Author, Labels, Milestones, Assignee, Sort; full-text search; open/closed counts | `/{o}/{r}/issues` header | REST issues + search | issue list filters, search and milestone page |
| Pull request list filters: Author, Label, Reviews (none / required / approved / changes requested / awaiting you), Assignee, search | `/{o}/{r}/pulls` | search | pull request list filters |
| Assignees editing | Issue / PR sidebar → Assignees ⚙ (and "assign yourself") | REST assignees | assignees and milestone editing from the sidebar; request a reviewer and assign on a PR |
| Milestone editing | Issue / PR sidebar → Milestone ⚙ | REST update issue | assignees and milestone editing from the sidebar |
| Projects of an issue / PR (add/remove) | Issue / PR sidebar → Projects ⚙ | GraphQL `projectItems`, `addProjectV2ItemById`, `deleteProjectV2Item` | issue sidebar projects… |
| Sub-issues: progress, create sub-issue, add existing, remove; parent link | Issue page "Sub-issues" box; sidebar Relationships → Add parent | REST `/sub_issues`, `/sub_issue`, `/parent` (new on the server) | sub-issues (create and add existing) and parent link |
| Dependencies: blocked by / blocking, add, remove | Issue sidebar → Relationships ⚙ | REST `/dependencies/blocked_by`, `/blocking` | dependencies (blocked by / blocking) |
| Reactions on issue / PR bodies, comments and review comments | Smiley button under each comment; click a reaction to toggle | REST reactions | reactions on issue and comment; reply to a thread, react… |
| Comments: edit, delete, quote reply, copy link, pin / unpin (pinned note at the top) | "…" menu on each comment | REST issue comments, `/pin` | issue title edit, comments (edit, quote, pin, delete) and timeline |
| Title and body editing | Edit button in the title row; "…" → Edit on the body | REST update | issue title edit… |
| Timeline events (labeled, assigned, milestoned, renamed, closed as …, reopened, locked, referenced, cross-referenced, review requested, review dismissed, ready for review, draft, base changed, force-pushed, pinned, transferred, …) | Issue / PR conversation | REST `/timeline` | issue title edit… (renamed, labeled, assigned, milestoned); close as not planned…; convert to draft… |
| Close as completed / not planned / duplicate, reopen | Close button with reason menu under the comment box | REST update `state_reason` | close as not planned, reopen, lock and unlock, pin |
| Lock / unlock conversation (with reason), locked banner, comment box hidden for non-collaborators | Issue / PR sidebar → Lock conversation | REST `/lock` | close as not planned, reopen, lock and unlock, pin |
| Pin / unpin issue | Issue sidebar | GraphQL `pinIssue`/`unpinIssue` | close as not planned, reopen, lock and unlock, pin |
| Transfer issue | Issue sidebar → Transfer issue | GraphQL `transferIssue` | transfer and delete issues |
| Delete issue (admins) | Issue sidebar → Delete issue | GraphQL `deleteIssue` | transfer and delete issues |
| Request / remove reviewers, review status per reviewer | PR sidebar → Reviewers ⚙ | REST `requested_reviewers` | request a reviewer and assign on a PR |
| Reviews: comment / approve / request changes, with summary | "Review changes" (Conversation and Files changed) | REST reviews | second user approves, owner dismisses the review; line comment… submit review |
| Dismiss a review (with message) | "Dismiss review" on approved / changes-requested reviews | REST `/dismissals` | second user approves, owner dismisses the review |
| Line comments, multi-line comments (click + then shift-click +), suggestions ("± Suggest"), single comment or pending review ("Start a review" → "Finish your review") | Files changed tab | REST review comments / create review with `comments[]` | line comment, multi-line suggestion in a pending review, submit review |
| Review threads in the conversation (file, line range, hunk), replies, outdated badge | Conversation tab, under each review | REST review comments | reply to a thread, react, resolve and unresolve |
| Resolve / unresolve conversations | "Resolve conversation" on a thread | GraphQL `reviewThreads`, `resolveReviewThread` (new on the server) | reply to a thread, react, resolve and unresolve |
| Commit a suggestion | "Commit suggestion" in a suggested change (same-repository PRs, push access) | REST contents (PUT on the head branch) | commit a suggestion |
| Draft ↔ ready for review | "Convert to draft" in the PR sidebar; "Ready for review" in the draft box | GraphQL `convertPullRequestToDraft`, `markPullRequestReadyForReview` | convert to draft and mark ready for review |
| Edit base branch | "Edit base" next to the PR meta line | REST update pull `base` | change the base branch and update the branch |
| Update branch (when behind the base) | Merge box → "Update branch" | REST `/update-branch` | change the base branch and update the branch |
| `/issues` dashboard: Created / Assigned / Mentioned, open/closed, search | Header nav → Issues | search | /pulls dashboard review requests … and /issues dashboard |
| `/pulls` dashboard: Created / Assigned / Mentioned / Review requests | Header nav → Pull requests | search (`review-requested:` is new on the server) | /pulls dashboard review requests (second user) and /issues dashboard |
| Projects list (user): open/closed, search, New project; links to the user's organizations' projects | Profile → Projects tab, `/users/{u}/projects`, avatar menu → Your projects | GraphQL `projectsV2`, `createProjectV2` | create a project from the profile and add items |
| Projects list (organization) | `/orgs/{o}/projects` | GraphQL | (not driven: needs an organization; see below) |
| Projects list (repository): linked projects, Link a project, New project (linked) | Repository → Projects tab | GraphQL `Repository.projectsV2`, `linkProjectV2ToRepository` | issue sidebar projects, repo Projects tab link… |
| Project table view: items (issues, PRs, drafts), inline field editing (single select, iteration, text, number, date), item menu (archive, delete) | `/{users\|orgs}/{login}/projects/{n}/views/{v}` | GraphQL items, `updateProjectV2ItemFieldValue`, `clearProjectV2ItemFieldValue` | create a project…; project fields … and values |
| Add items: draft issue by title, issue/PR by `owner/repo#n` or URL | "+ Add item" row | GraphQL `addProjectV2DraftIssue`, `addProjectV2ItemById` | create a project from the profile and add items |
| Board view: columns from the view's group-by field or Status, drag and drop or the card's column select | Board views | GraphQL; REST create view | board view, move a card, edit a draft, archive |
| Views: tabs, New view (table / board) | View tabs on the project page | GraphQL `ProjectV2.views` (new on the server), REST `POST …/views` | board view… |
| Item dialog: draft title/body editing, all field values, open issue/PR | Click an item title | GraphQL `updateProjectV2DraftIssue` | board view, move a card, edit a draft, archive |
| Archived items toggle, archive / restore | "Archived items" checkbox; item menu | GraphQL `archiveProjectV2Item` | board view, move a card, edit a draft, archive |
| Project settings: name, description, README, custom fields (text, number, date, single select with options, iteration with start/duration), delete field, linked repositories, visibility, close / re-open, delete | Project → Settings | GraphQL `updateProjectV2`, `createProjectV2Field`, `deleteProjectV2Field`, `deleteProjectV2` | project fields …; issue sidebar projects… close and delete a project |
| Discussions tab (shown when the repository has discussions) | Repository → Discussions | GraphQL | discussions — create, comment, reply, answer, close, lock, delete |
| Discussion list with categories sidebar, open/closed, per category | `/{o}/{r}/discussions`, `/discussions/categories/{slug}` | GraphQL `discussions`, `discussionCategories` | discussions … |
| New discussion (choose a category) | `/{o}/{r}/discussions/new` | `createDiscussion` | discussions … |
| Discussion page: comments, threaded replies, edit/delete comments, mark / unmark as answer, edit title/body, change category, close (resolved / outdated / duplicate), reopen, lock / unlock, delete | `/{o}/{r}/discussions/{n}` | GraphQL discussion mutations; `lockLockable` (Discussions accepted on the server now) | discussions … |
| Permissions: read-only users see no edit controls (labels, milestones, sidebar gears, lock, project settings) | everywhere | — | non-collaborators get read-only planning pages |
| Direct URLs (hard reload) | `/issues`, `/pulls`, `/users/{u}/projects`, project, labels, milestones, repo projects, discussions | — | direct URLs render (hard reload) |
| 375 px layout | issues, issue, labels, milestones, PR, PR files, dashboards, PR list, project table/board/list, discussions | — | 375px pages have no horizontal overflow |

## Server changes made for the UI

All in `crates/forge`, tested by `tests/planning.rs`:

- Sub-issues REST endpoints (api.github.com shape; migration `1581_sub_issues.sql`).
- GraphQL `PullRequest.reviewThreads`, `PullRequestReviewThread`, `PullRequestReviewComment`,
  `resolveReviewThread`, `unresolveReviewThread` (migration `1582_review_thread_resolutions.sql`).
- GraphQL `ProjectV2.views` / `view(number:)`.
- Issue search qualifiers `review-requested`, `user-review-requested`, `review:`, `archived:`.
- `review_requested` timeline events carried the wrong data keys (reviewer showed as ghost).
- Issue comments include `pin` in lists (GHES schema field).
- `lockLockable` / `unlockLockable` accept Discussions.
- `Repository.issueTemplates` reads `.github/ISSUE_TEMPLATE/*.md` (was always empty).
- `transferIssue` returned the issue's old number (stale dataloader row).

## Left out

- **Auto-merge**: the server does not implement it (`enablePullRequestAutoMerge` always fails
  with "Auto merge is not allowed", `auto_merge` is always null); building it needs a merge
  trigger on check/review completion, which is server work beyond this area.
- **Organization projects in the e2e**: the pages and routes exist (`/orgs/{o}/projects…`, same
  code as user projects), but the e2e user can't create an organization on a shared database
  without site-admin rights, so only user and repository projects are driven. The org profile
  page belongs to the orgs area, so a Projects link there is theirs to add (the URL is
  `/orgs/{org}/projects`); users reach it from their own Projects list ("Organization
  projects: …").
- **Roadmap layout**: roadmap views render as tables (the server stores the layout, but there
  are no date-range fields to lay out).
- **Discussion reactions / upvotes and category management**: the server's discussion
  `reactionGroups` are stubs and GitHub has no API to edit categories.
- **YAML issue forms** (`.yml` templates) and `config.yml` contact links: only Markdown
  templates are parsed.
- **Team review requests**: forge has no team review requests.
- **Pending reviews survive only in the browser session** (sessionStorage), because the REST API
  can't add comments to an existing pending review; they are submitted together with the review.

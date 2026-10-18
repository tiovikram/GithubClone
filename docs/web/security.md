# Web UI: security area

Code: `web/src/areas/security/`. E2E: `web/e2e/areas/security.mjs` (25 steps incl. the shared
sign-up / token steps). Server additions: `crates/forge/src/advsec/repo_advisories.rs` (+ migration
`1560_repository_advisories.sql`), dependency review vulnerabilities in
`api/dependency_graph.rs` / `advsec/advisories.rs`, delegated keys in `advsec/mod.rs`
(`apply_security_and_analysis`). Tests: `crates/forge/tests/repo_advisories.rs`,
`dependency_review_reports_vulnerabilities` and `security_and_analysis_accepts_delegated_settings`
in `crates/forge/tests/advsec.rs`.

The e2e is offline: when the e2e user is a site admin it imports the OSV fixture
(`crates/forge/tests/fixtures/advisories/osv-sample.json`) through `POST /api/forge/advisories`;
otherwise it uses advisories already present (or skips the Dependabot steps with a `SKIP` line).
Secrets are random fake AWS key ids; code scanning results are a small SARIF file uploaded through
the UI.

## Features

| Feature | UI location | E2E step |
|---|---|---|
| Security tab (order 60) and overview (policy, advisories, private reporting, alert counts) | `/{o}/{r}/security`, left nav Overview / Reporting / Vulnerability alerts / Supply chain | tab and overview |
| Security policy: SECURITY.md detection (`/`, `.github/`, `docs/`), rendered; create / edit and commit from the page | `/{o}/{r}/security/policy` | create a security policy |
| Repository security advisories: list (Published / Draft / Triage / Closed) | `/{o}/{r}/security/advisories` | draft advisory…, private vulnerability report… |
| Create draft advisory (title, CVE id, description, affected products, severity or CVSS vector, CWEs, credits, collaborators) | `…/security/advisories/new` (admins) | draft advisory with credits, CVE request, publish |
| Edit advisory, credits (pending / accepted), request CVE, publish, close, reopen | `…/security/advisories/GHSA-…` | draft advisory…; private vulnerability report… (close, reopen button) |
| Private vulnerability reporting toggle | Settings → Advanced Security | Advanced Security settings |
| Private vulnerability report form (non-maintainers), triage: accept as draft / close | `…/security/advisories/new` (reporters), triage banner on the advisory | private vulnerability report accepted and closed |
| Dependabot alerts list: open / closed, severity, ecosystem, scope, package, manifest filters, sort, pagination | `…/security/dependabot` | Dependabot alerts list and filters |
| Dependabot alert detail, dismiss with reason + comment, reopen | `…/security/dependabot/N` | Dependabot alert dismiss and reopen |
| Dependabot dismissal requests (delegated dismissal): request on dismiss, reviewer approve / deny, requester cancel | request box on the alert page | not driven (see below) |
| Code scanning alerts list: open / closed, tool, branch, severity filters | `…/security/code-scanning` | code scanning SARIF upload… |
| Code scanning alert detail: location, code snippet at the analysed commit, message, rule help, tags, affected branches; dismiss with reason / comment, reopen; dismissal requests (`create_request` when delegated) | `…/security/code-scanning/N` | code scanning SARIF upload, alert dismiss and reopen |
| Code scanning tools: analyses list (tool filter, delete with confirm for the last of a set), SARIF upload (gzip + base64 in the browser) with processing status, CodeQL default setup | `…/security/code-scanning/tools` | code scanning SARIF upload…; delete a code scanning analysis; Advanced Security settings (default setup) |
| Secret scanning alerts list: open / closed, secret type, resolution, validity, bypassed-only; scan history | `…/security/secret-scanning` | secret scanning alert resolve and reopen |
| Secret scanning alert: reveal secret, locations (commit / file links), close as (false positive, used in tests, won't fix, revoked) with comment, reopen, closure requests | `…/security/secret-scanning/N` | secret scanning alert resolve and reopen |
| Push protection unblock page (the URL in GH013 rejections) | `…/security/secret-scanning/unblock-secret/{placeholder}` | push protection blocks a push, unblock page allows it |
| Push protection bypasses: bypassed secrets and delegated bypass requests; request page with approve / reject | `…/security/secret-scanning/push-protection-bypasses[/N]` | push protection…; delegated bypass request reviewed by the owner |
| Dependency graph per ecosystem with search and links to alerts; SBOM (SPDX 2.3) export download | `/{o}/{r}/network/dependencies` (Security nav → Dependency graph) | dependency graph and SBOM export |
| Dependency review on PRs: added / removed dependencies with known vulnerabilities | slot `pull.merge.top` on the PR conversation | dependency review on a pull request |
| Repository "Advanced Security" settings: private reporting, Dependabot alerts, security updates, GHAS, CodeQL default setup, secret scanning, push protection, non-provider patterns, validity checks, delegated dismissal, delegated bypass (+ team reviewers for org repos); attached / enforced configuration notice | `/{o}/{r}/settings/security_analysis` (group Security) | Advanced Security settings; delegated bypass… |
| Repository Dependabot secrets (sealed box in the browser) | `/{o}/{r}/settings/secrets/dependabot` | repository Dependabot secrets |
| Org code security configurations: list (incl. GitHub recommended / enterprise), create, edit, delete, apply to repositories (all / without configuration / public / private / selected), default for new repositories, attached repositories | `/organizations/{org}/settings/security_products[/configurations/new\|/{id}/edit]` | organization code security… |
| Org push protection pattern settings | same page, "Push protection patterns" | organization code security… (rendered) |
| Org security overview: Dependabot / code / secret alerts across repos, repository advisories, dismissal and bypass requests | `/organizations/{org}/settings/security_overview` | organization code security… |
| Org Dependabot secrets (all / private / selected repositories) and Dependabot repository access | `/organizations/{org}/settings/secrets/dependabot` | organization code security… |
| Enterprise code security: defaults for new repositories, push-protection custom link, enterprise configurations | `/site-admin/code-security` | site admin code security |
| Advisory database browser: search (package, `pkg@version`, GHSA, CVE, CWE), type tabs, ecosystem / severity filters; advisory page | `/advisories`, `/advisories/GHSA-…` (user menu → Advisory database; advisory links on alerts) | advisory database browser; Dependabot alert…; draft advisory… (published advisory appears there) |
| 375 px layout | all main pages above | pages fit a 375px screen |

The org and site admin steps navigate to the shells owned by the orgs and admin areas and log
`SKIP` when the shell (settings sidebar plus the section) is not rendered, or when the e2e user is
not a site admin. They were run green against local, uncommitted stand-in shells while those
areas were in progress.

## Server changes

- Repository security advisories and private vulnerability reporting (api.github.com shapes; not
  part of GHES 3.22's REST surface): `GET/POST /repos/{o}/{r}/security-advisories`,
  `GET/PATCH …/{ghsa_id}`, `POST …/reports`, `POST …/{ghsa_id}/cve` (202; recorded as
  `cve_requested_at` since forge is not a CNA), `POST …/{ghsa_id}/forks` (422, not supported),
  `GET /orgs/{org}/security-advisories`, `GET/PUT/DELETE /repos/{o}/{r}/private-vulnerability-reporting`.
  Publishing imports the advisory into the global database so Dependabot matches it.
- `dependency-graph/compare` fills `vulnerabilities` from the advisory database (was always empty).
- `PATCH /repos/{o}/{r}` `security_and_analysis` accepts GHES's `secret_scanning_delegated_alert_dismissal`,
  `secret_scanning_delegated_bypass` and `secret_scanning_delegated_bypass_options` (were ignored);
  the repository JSON shows the reviewers.

## New npm dependencies

`tweetnacl@1.0.3` and `blakejs@1.2.1` (pinned exactly) for libsodium sealed boxes
(`sealedbox.ts`), the same pair the conformance harness uses for Actions / Dependabot secrets.

## Left out

- Dependabot / code scanning dismissal requests are implemented in the UI but not driven by the
  e2e: delegated dismissal for Dependabot and code scanning can only be enabled through an
  organization code security configuration, which needs the org shell plus a second org member with
  write access. Secret scanning delegated bypass (same request UI component) is driven end to end.
- Custom secret scanning patterns: not part of GHES 3.22's REST API and not implemented by the
  server (`docs/advsec.md`).
- Temporary private forks for advisories: the server answers 422.
- Running CodeQL: the server stores default setup but has no runners; the UI says so.
- Dependabot security update / version update pull requests: not implemented by the server; the
  toggle only stores the setting.
- A SARIF upload history list: the API has no list endpoint; uploads show their status after
  submission, and each analysis shows its `sarif_id`.

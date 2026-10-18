# Advanced security (advsec area)

Code: `crates/forge/src/advsec/`. Migration `1450_advsec.sql`. Tests `crates/forge/tests/advsec.rs`.
Conformance scenarios `tools/conformance/scenarios/write-advsec.mjs`. Covers the REST tags
`secret-scanning`, `dependabot`, `code-scanning`, `code-security` and `security-advisories`
(90 operations in GHES 3.22).

## Enablement

A feature acts only when it is enabled for the repository. Flags live in the orgs area's
`repo_security_products` (one row per product), and Dependabot alerts in the repos area's
`repo_settings.vulnerability_alerts`. They are set by:

- `PATCH /repos/{o}/{r}` with `security_and_analysis` (`{"secret_scanning": {"status": "enabled"}}`,
  also `secret_scanning_push_protection`, `secret_scanning_non_provider_patterns`,
  `secret_scanning_validity_checks`, `advanced_security`, `dependabot_security_updates`); the
  repository JSON shows `security_and_analysis` to admins;
- `PUT/DELETE /repos/{o}/{r}/vulnerability-alerts` (Dependabot alerts);
- `POST /orgs/{org}/{product}/enable_all|disable_all` (orgs area; forge then starts scans);
- attaching a code security configuration (below).

Defaults are GHES's: everything off except the dependency graph. In particular **push protection
is off** unless enabled, so pushes of existing fixtures are never blocked. Turning secret scanning
on schedules a backfill scan; turning Dependabot alerts on schedules a match.

| product | effect |
|---|---|
| `secret_scanning` | scans pushes, backfill on enable; alerts API answers (404 while off) |
| `secret_scanning_push_protection` | rejects pushes / contents API writes with unbypassed secrets |
| `secret_scanning_non_provider_patterns` | also scans connection strings and HTTP auth headers |
| `secret_scanning_delegated_bypass` (+ reviewers) | bypasses by non-reviewers become bypass requests |
| `*_delegated_alert_dismissal` (secret scanning, code scanning, Dependabot) | closing alerts goes through dismissal requests |
| `dependabot_alerts` | Dependabot matching; alerts API answers (403 while off) |
| `code_scanning_default_setup` | stored with the default setup configuration |

## Secret scanning

- **Patterns** (`patterns.rs`): GitHub's `secret_type` slugs for GitHub tokens (`ghp_`, `gho_`,
  `ghs_`, `ghu_`, `ghr_`, `github_pat_`; forge's own tokens use these formats), AWS key ids and
  secret keys, Slack tokens and webhooks, Stripe, Google API keys and OAuth secrets, npm, PyPI,
  Docker, GitLab, SendGrid, Twilio, Shopify, OpenAI, Anthropic, DigitalOcean, HashiCorp Vault,
  Mailchimp, Telegram, Azure storage keys, and PEM/OpenSSH/PGP private keys. Non-provider patterns
  (Postgres/MongoDB/MySQL connection strings, HTTP basic/bearer headers) only with
  `secret_scanning_non_provider_patterns`. Custom patterns are not part of GHES 3.22's REST
  surface and are not implemented.
- **Incremental scans.** `hooks::after_refs_changed` (every push, contents API write, merge)
  queues `advsec.secret_scan` on the Postgres job queue. The job scans only commits new to the
  repository (`git log <new> --not <old> <other branches/tags>`, at most 1000 commits), each
  changed blob once, blobs up to 1 MiB, text only (no NUL in the first 8000 bytes).
- **Backfill** when secret scanning is enabled: all branches and tags, at most 10000 commits.
- **Alerts** are unique per (repository, secret type, sha256 of the secret); each (commit, path,
  line, column) where it appears is a location (`.../alerts/{n}/locations`). Columns are 1-based,
  end exclusive. Resolutions: `false_positive`, `wont_fix`, `revoked`, `used_in_tests` (422 without
  one); reopening clears them. Assignees need write access. Scan history:
  `.../secret-scanning/scan-history` (`incremental_scans`, `backfill_scans`).
- **Forge tokens.** A `gh?_` token found is looked up in the token table: validity `active` /
  `inactive`. In a **public** repository forge revokes it immediately (the token row is deleted)
  and resolves the alert as `revoked` with a comment — GitHub does the same for its own tokens.
- **Push protection.** For git pushes (HTTP and SSH) forge points `git receive-pack` at a
  generated `pre-receive` hook (`$FORGE_CACHE_DIR/.forge-advsec-hooks/pre-receive`) that first
  runs the branch protection hook (if any) and then `forge hook pre-receive` — the forge binary
  itself (`FORGE_HOOK_BIN` overrides; without a forge binary push protection is skipped with a
  warning). The hook scans the incoming commits (`<new> --not --all`, quarantined objects
  included) without database access, using a per-push rules file (protected token types, active
  bypasses), and rejects the push with GitHub's `GH013` report: each secret with its commit,
  path and line and an unblock URL ending in the **placeholder id** (a deterministic hash of
  repository, type and secret). Contents API writes are checked in-process and answer 409 with
  `errors[].placeholder_id` / `token_type`.
  Token types follow the patterns' defaults overridden by the enterprise and organization
  *pattern configurations* (`.../secret-scanning/pattern-configurations`, optimistic
  concurrency through `pattern_config_version`, 409 on mismatch).
- **Bypasses.** `POST .../secret-scanning/push-protection-bypasses {reason, placeholder_id}`
  (write access; 404 for placeholders never blocked) allows that secret for 3 hours. The alert
  created for it records `push_protection_bypassed`, `_by`, `_at`. With delegated bypass the
  request of a non-reviewer becomes a *bypass request* (403 with its URL) that reviewers approve
  or reject (`.../bypass-requests/secret-scanning/{n}`); approving grants the bypass, dismissing
  the response (`DELETE .../bypass-responses/secret-scanning/{id}`) withdraws it and reopens the
  request.

## Dependabot

- **Advisory database.** `advisories` + `advisory_vulnerabilities`, imported from OSV records of
  the GitHub Advisory Database: `forge advisories sync --from <dir|file.json|file.zip|https://…>`
  (a checkout of github/advisory-database, or osv.dev's `gs://osv-vulnerabilities/<ecosystem>/all.zip`
  — e.g. `https://osv-vulnerabilities.storage.googleapis.com/crates.io/all.zip`, ~3.6 MB, 2484
  advisories), or the site-admin-only `POST /api/forge/advisories` (one OSV record or an array;
  used by tests and the conformance scenario with `crates/forge/tests/fixtures/advisories/osv-sample.json`).
  Records without a GHSA id are imported under their GHSA alias; GitHub's own record wins over an
  alias record. OSV ranges become GitHub's `vulnerable_version_range` (`>= 1.0.0, < 1.2.6`), one
  vulnerability per interval. `GET /advisories` (filters: `ghsa_id`, `type`, `cve_id`,
  `ecosystem`, `severity`, `cwes`, `is_withdrawn`, `affects` with `pkg@version`, `published`,
  `updated`, `modified` date ranges, sorting, cursors) and `GET /advisories/{ghsa_id}`.
- **Matching** (`advsec.dependabot` job) after pushes to the default branch, when alerts are
  enabled, and for every enabled repository after an import: the dependency graph of the default
  branch (`api::dependency_graph::graph`: manifests + submitted snapshots) against the advisories
  (version comparison tolerant of `v` prefixes, pre-releases, pip/Maven segment styles). States:
  `open`; `fixed` when the manifest no longer has a vulnerable version (reopens if it comes back);
  `dismissed` with a reason (`fix_started`, `inaccurate`, `no_bandwidth`, `not_used`,
  `tolerable_risk`); `auto_dismissed` by the preset rule for npm *development* dependencies whose
  advisory is only resource exhaustion / ReDoS (CWE-400, 409, 770, 1333).
- **Dismissal requests** (`POST/GET/PATCH/DELETE .../dismissal-requests/dependabot/{n}`) need
  `dependabot_delegated_alert_dismissal`; non-reviewers then cannot dismiss directly.
- **Secrets** (`/repos/…/dependabot/secrets`, `/orgs/…/dependabot/secrets`): libsodium sealed
  boxes like Actions secrets, with separate key pairs (`actions_public_keys` owner kinds
  `dependabot-repo` / `dependabot-org`, private halves sealed with the master key). Values that do
  not open are 422. `advsec::dependabot_secrets::secrets_for_update(state, repo_id)` decrypts them
  (repository secrets override organization secrets visible to the repository).
- **Repository access** (`/orgs/{org}/dependabot/repository-access`): repositories Dependabot may
  read for private dependencies (org repositories or public ones) and the default level.

## Code scanning

- `POST .../code-scanning/sarifs` takes gzip+base64 SARIF 2.1.0 (≤ 10 MiB compressed, ≤ 100 MiB
  uncompressed, ≤ 20 runs, ≤ 25000 results per run; 400/413 otherwise), stores it in object
  storage (`advsec/sarif/{repo}/{id}.sarif.gz`) and returns 202; `advsec.sarif` processes it
  (`.../sarifs/{id}`: `pending` → `complete` / `failed` with errors).
- Each run is an **analysis** in a set keyed by (ref, tool, category = `automationDetails.id` up
  to its last `/`). Results become **alerts** deduplicated by (tool, rule, fingerprint):
  `partialFingerprints.primaryLocationLineHash` or another fingerprint when given, else a hash of
  rule, path and the source line at the analysed commit (so moving code does not create new
  alerts). Rules carry `severity` (SARIF level) and `security_severity_level` (from
  `security-severity`).
- In each set the latest analysis decides: an alert it contains is open there, an alert it lacks
  is fixed. An alert is `open` while any set has it open, otherwise `fixed`; dismissal (`false
  positive`, `won't fix`, `used in tests`, `mitigated`) overrides; reopening re-evaluates.
  `.../alerts/{n}/instances` lists one instance per set.
- Analyses: list/get (also as `application/sarif+json`, the stored run) and delete: only the
  latest analysis of a set is deletable; deleting the last one of a set needs `confirm_delete`;
  the response links the next deletable analysis. Alerts left without results are removed.
- Delegated dismissal: `PATCH .../alerts/{n}` with `create_request: true` opens a dismissal
  request (`.../dismissal-requests/code-scanning/{n}`); approval dismisses it and records
  `dismissal_approved_by`.
- Default setup (`.../code-scanning/default-setup`) is stored (state, languages — detected from
  the default branch when omitted —, runner, query suite, threat model). forge has no hosted
  CodeQL runners, so no analysis run is started (200, not 202).

## Code security configurations

Organization configurations (`/orgs/{org}/code-security/configurations`, owners and security
managers), enterprise configurations (`/enterprises/{e}/…`, site admins) and the seeded global
"GitHub recommended" configuration (`target_type: global`, read-only). Attaching (`scope`: `all`,
`all_without_configurations`, `public`, `private_or_internal`, `selected`) applies every
`enabled`/`disabled` setting to the repositories (and starts scans); `not_set` leaves them alone;
updating a configuration re-applies it; detaching keeps the settings. `enforced` configurations
lock `security_and_analysis` on their repositories (422). Defaults for new repositories
(`.../{id}/defaults`, per visibility; organization defaults before enterprise ones) are applied
when a repository is created.

## Requests and permissions

- Alerts: secret scanning needs repository admin; Dependabot write; code scanning write (read
  for public repositories). Organization owners and holders of the product's fine-grained
  permissions through organization roles (the predefined security manager role has
  `read/write_secret_scanning`, `…_code_scanning`, `…_dependabot_alerts`) always qualify;
  organization lists need those permissions; enterprise lists are site-admin only and cover
  organization-owned repositories.
- Request reviewers: organization owners, security managers, holders of the product's write
  permission, and the configuration's listed reviewers (teams / organization roles); on
  user-owned repositories, the repository admins. Requests expire after 7 days.
- Webhooks: `secret_scanning_alert`, `dependabot_alert`, `code_scanning_alert` events are queued
  for repository hooks (created / resolved / reopened / dismissed / fixed).

## Not implemented

- Custom secret scanning patterns, validity checks against third-party providers (only forge's
  own tokens are checked), and the AI/generic secret detector.
- Dependabot security/version update pull requests, and actually running CodeQL default setup.

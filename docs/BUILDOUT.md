# Build-out: the whole GHES 3.22 REST surface

Goal: every one of the 1105 operations in `tools/conformance/spec/ghes-3.22.json` is implemented
**with real behaviour** (data persisted in Postgres/object storage, side effects applied, permissions
enforced the way GitHub documents them). Each one must also return JSON that conforms to the
schema and be exercised by a conformance scenario.

`docs/coverage-missing.json` lists the 704 operations not routed at the start, grouped by tag.
The list comes from a static scan, so a few entries may already be routed. Check before
re-implementing.

Read `docs/AGENTS.md` (team rules) and `docs/ARCHITECTURE.md` (the contract) first.

## Areas, owners and resources

| Area | Tags in coverage-missing.json | Branch author | DBs | Ports (dev / gh suite) | Migrations | Wave |
|---|---|---|---|---|---|---|
| checks | checks, reactions, plus untested tier-2 scenarios | forge-checks | forge_checks* | 3131 / 3132 | 0950-0969 | 1 |
| security | (docs/security.md known gaps) | forge-security | forge_sec2* | 3141 / 3142 | 0970-0989 | 1 |
| runner | actions: workflows, runs, jobs, logs, artifacts, caches, self-hosted runners and registration; oidc | forge-runner | forge_runner* | 3161 / 3162 | 1150-1199 | 1 |
| repos | repos, users, issues, gists, activity, codes-of-conduct, dependency-graph | forge-repos | forge_repos* | 3171 / 3172 | 1200-1249 | 1 |
| orgs | orgs, teams, private-registries | forge-orgs | forge_orgs* | 3181 / 3182 | 1250-1299 | 1 |
| enterprise | enterprise-admin, enterprise-teams, enterprise-team-memberships, enterprise-team-organizations, billing | forge-enterprise | forge_ent* | 3151 / 3152 | 1100-1149 | 2 |
| actions-settings | actions: secrets, variables, permissions, runner groups, retention, org/enterprise-level settings | forge-actions | forge_actset* | 3191 / 3192 | 1300-1349 | 2 |
| apps | apps, oauth-authorizations, migrations | forge-apps | forge_apps* | 3201 / 3202 | 1350-1399 | 2 |
| packages | packages (with a real OCI container registry and npm registry on object storage), projects (classic) | forge-packages | forge_pkg* | 3211 / 3212 | 1400-1449 | 2 |
| advsec | dependabot, secret-scanning, code-security, code-scanning, security-advisories | forge-advsec | forge_advsec* | 3221 / 3222 | 1450-1499 | 2 |

`forge_x*` means `forge_x` (dev), `forge_x_test` (cargo test), `forge_x_gh` (gh suite).

## Rules specific to the build-out

- **Conformance scenarios:** put them in your own file, `tools/conformance/scenarios/write-<area>.mjs`
  (exporting `scenarios`). run.mjs loads every `write-*.mjs` file after the core scenarios and
  before `cleanup`. Only edit `write.mjs` to fix a genuine bug. Scenarios must clean up after
  themselves, and must not depend on other areas' scenarios except through the shared state the
  core scenarios set up (`s.login`, `s.repo`, `s.org`, …; read `write.mjs`).
- **Code:** put real code in your own modules (`api/<area>_*.rs`, `<area>/`). Keep shared files
  (`lib.rs`, `api/mod.rs`, `json/mod.rs`, `Cargo.toml`, `main.rs`) additive.
- **Done means:**
  - every operation in your tags is routed and conforming in the harness (or "blocked" with
    a written reason in your report);
  - `cargo test -p forge` passes;
  - `tests/gh/run.sh` is still 166/166;
  - tier 1/2 has not regressed.
- **Machine:** 10 cores and 24 GB shared by 5 agents. Build with `CARGO_BUILD_JOBS=3`. Wrap long
  commands in `caffeinate -dims` (the Mac idle-sleeps otherwise, which pauses everything).
  Kill only your own processes.
- **Costs:** nothing that costs money or touches an account. Only free, public, anonymous
  downloads are allowed (e.g. a container base image, a public advisory dataset). Record any new
  dependency so `scripts/uninstall-deps.sh` can remove it.
- **Report:** finish with operations implemented, conformance numbers for your tags plus
  tier 1/2/all, tests, shared-file edits, and what is blocked and why.

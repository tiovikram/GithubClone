# REST API conformance oracle

`tools/conformance` checks a server against GitHub's published OpenAPI description
([github/rest-api-description](https://github.com/github/rest-api-description)). It runs
realistic scenarios that build their own fixtures (repo → contents → refs → issues → PR → merge →
release …), validates every response against the documented status codes and response schemas,
and reports a score: % of spec operations that conform, overall, per priority tier, and per tag.
This is success criterion 3 in [ARCHITECTURE.md](ARCHITECTURE.md).

Requirements: Node ≥ 20. The only dependencies are `ajv` and `ajv-formats`.

```sh
cd tools/conformance
npm install
node fetch-spec.mjs          # download the pinned spec into spec/ (gitignored); verifies checksums
npm test                     # offline validator regression test (no network)
```

## Running against a server

```sh
# our forge (GHES-style base path)
forge admin create-user --login conf --password 'x' --admin
TOKEN=$(forge admin create-token --login conf)
node run.mjs --base http://localhost:3000/api/v3 --token "$TOKEN"            # all scenarios
node run.mjs --base http://localhost:3000/api/v3 --token "$TOKEN" --tier 1   # gh-CLI core only
node run.mjs --base http://localhost:3000/api/v3 --token "$TOKEN" --report out.json --md out.md

# baseline: Gitea / Forgejo (only the base path differs)
node run.mjs --base https://gitea.example.com/api/v1 --token "$GITEA_TOKEN" --md reports/baseline-gitea.md

# self-test / calibration against real GitHub, read-only, no token (≤ 30 requests)
npm run selftest
```

Useful options (`node run.mjs --help` lists all):

| option | meaning |
|---|---|
| `--spec ghes\|dotcom\|FILE` | which description to validate against. Default `ghes` (GHES 3.22, matches our `/api/v3`); `dotcom` = api.github.com |
| `--tier N` | only exercise operations of tier ≤ N. Fixture setup calls (create repo, refs, …) always run |
| `--org ORG` | an org the token user owns; enables org scenarios (`orgs/*`, `repos/create-in-org`, fork + `merge-upstream`) |
| `--collaborator LOGIN` | a second user; enables `add/remove-collaborator`, `request/remove-requested-reviewers` |
| `--collaborator-token T` | that user's token (or env `CONFORMANCE_COLLABORATOR_TOKEN`); scenarios act as them with `t.call(op, {token: s.collaboratorToken})`, e.g. an approving review for `pulls/dismiss-review` |
| `--only REGEX` | run only matching scenarios (identity, repo setup and cleanup always run) |
| `--keep` | don't delete the fixture repo `conf-<runId>` at the end |
| `--auth-scheme Bearer` | default is `Authorization: token …` (accepted by GitHub, Gitea, Forgejo and us) |
| `--record F` / `--replay F` | record all HTTP exchanges / replay them offline (useful for debugging the validator) |
| `--strict-oneof` | keep OpenAPI `oneOf` strict (see quirks) |
| `--verbose` | print every call, not only failures |

Output: a console summary, `reports/latest.json` (full per-operation detail; gitignored) and
`reports/latest.md` (Markdown summary: score table, per-tag table, failing operations with their
first validation errors, untested tier 1/2 operations, scenario status, harness warnings).
Exit code is 0 unless the run itself crashed (scores are informational; gate on them in CI by
reading `summary.tier1.pct_conforming` from the JSON).

## What is measured

For every call the oracle records a verdict, and an operation's status is the worst verdict over
all its calls:

- **conforming** — status code is one the spec documents for that operation *and* is the one
  the scenario expected (e.g. `201` for a create; a documented `422` where `201` was expected is a
  failure); JSON bodies validate against the documented schema; `Content-Type` is JSON when the
  spec documents a JSON body; any `Link` header is well-formed; lists that the scenario knows are
  non-empty are non-empty (otherwise `[]` would trivially validate).
- **nonconforming** — any of the above fails; the first few errors are kept, e.g.
  `schema: /0/user missing required properties: login, id, …` or `unexpected status 422 …`.
- **not_implemented** — the route answered `404`, `405` or `501` although the scenario expected
  success (a resource the scenario just created can't 404). Calls that *expect* 404 — e.g.
  `pulls/check-if-merged` before merging, `check-repo-is-starred` after unstarring — are exempt.
- **untested** — no call was made. The report distinguishes *no scenario* from *scenario not
  reached* (blocked by a failed fixture step, filtered by `--tier`, or needs `--org`/`--collaborator`).

Score = conforming / all operations in the spec (also reported per tier and per tag, plus
"% of tested" to separate breadth from quality).

**Pagination.** For selected lists (`issues/list-for-repo`, `issues/list-comments`,
`issues/list-labels-for-repo`, `repos/list-commits`) the runner requests `per_page=1` when ≥ 2 items
exist and requires `Link: <…>; rel="next"`, follows it (same origin as `--base`), validates page 2
against the same schema, checks it returned a different item and carries `rel="prev"`. Any `Link`
header on any response must parse, use only `next|prev|first|last`, have absolute URLs on the API
origin and numeric `page` values.

**Fixtures.** Write scenarios create `conf-<runId>` (with `auto_init`, MIT license, Node gitignore),
then: topics, contents API create/update/delete, git data API (blob → tree → commit → ref, update
ref, annotated tag), branch rename/merge/protection, commit statuses and commit comments, labels
and milestones, issues (assignees, labels, lock, events, timeline), comments and reactions, a PR
with review comments, replies, reviews (pending → delete, pending → submit), a second PR that is
behind its base (`update-branch`), merge, releases with an uploaded asset (uploaded to the
`upload_url` the server returns), deployments + statuses, webhooks + ping, stars, subscriptions,
collaborators, template generation, SSH keys (a fresh ed25519 key per run), gists, search, markdown,
and finally delete the repos. IDs/numbers are taken from responses. A scenario's essential steps
are marked `fixture` — if one fails the scenario stops (`blocked`) and later scenarios that need its
state are skipped. Where a GitHub-compatible fallback exists (e.g. building the feature branch via
the contents API when the git data API is missing) it is used so more of the surface gets measured.

**Harness self-check.** Every request body a scenario sends is validated against the operation's
request schema; mismatches appear under "Harness warnings" (a warning means the *scenario* is wrong).

## Priority tiers

`priority.json` lists tier 1 and tier 2 explicitly by operationId (grouped by area); everything else
is tier 3. `node catalogue.mjs` checks every listed id exists in the pinned spec and writes
`catalogue.{ghes,dotcom}.json` — the full operation catalogue (method, path template, operationId,
tag, tier, summary).

| spec | operations | tier 1 | tier 2 | tier 3 |
|---|---:|---:|---:|---:|
| GHES 3.22 (default) | 1105 | 104 | 129 | 872 |
| api.github.com | 1232 | 104 | 129 | 999 |

- **Tier 1 (104) — what `gh` needs from a forge.** The REST endpoints behind `gh repo
  create/view/clone/fork/edit/sync/delete`, `gh issue *`, `gh pr *` (create, view, merge, checkout,
  update-branch, reviewers), `gh release *` (incl. asset upload), `gh label *`, `gh search *`,
  `gh ssh-key` / `gh gpg-key`, `gh api` staples (`/`, `/meta`, `/rate_limit`, `/user`), plus the
  contents and git-refs endpoints that `gh` and every client use to read/write files and branches,
  milestones, issue comments, and basic org lookups. `gh` uses GraphQL for parts of issue/PR
  listing, but the REST equivalents are what other tools and our own compatibility claims rely on,
  so they are tier 1 too.
- **Tier 2 (129) — what @octokit/rest consumers commonly call.** Bots, CI integrations and
  `actions/github-script` users: the git data API (blobs, trees, commits, tags), commits/compare,
  commit comments, branch merge/rename/protection, archives, statuses and checks *reads*,
  collaborators, stars/watching/notifications/events, reactions, PR reviews and review comments,
  issue events/timeline, deployments, webhooks, gists, Actions read endpoints (`gh run`/`gh
  workflow` list/view), and small utility endpoints (markdown, emojis, licenses, gitignore, zen).
- **Tier 3 — everything else** (enterprise admin, Actions write/secrets, apps, code scanning,
  packages, projects, codespaces, copilot, …).

Scenario coverage today: all 104 tier 1 and 129 tier 2 operations are exercised by some scenario
(some need `--org`, `--collaborator` and `--collaborator-token`). Since there is no GitHub App in
forge, the Checks scenarios (`scenarios/write-checks.mjs`) create check runs with the user's
token (forge's built-in "forge" app; GitHub would answer 403 to a PAT). GPG keys are generated
offline (`gpgPublicKey()` in `write-checks.mjs`: Ed25519 key + self-signature via `node:crypto`).
Area scenario files `scenarios/write-<area>.mjs` run after the core scenarios and before cleanup.

## Spec source and pinning

`fetch-spec.mjs` downloads, from `raw.githubusercontent.com` (no API rate limit):

- `descriptions/ghes-<highest>/ghes-<highest>.json` (currently 3.22) — default validation target,
  since we serve the GHES-style `/api/v3` path;
- `descriptions/api.github.com/api.github.com.json` — used for the github.com self-test.

The upstream commit SHA, GHES version and SHA-256 of each file are pinned in
`tools/conformance/spec.lock.json` (committed). `node fetch-spec.mjs` re-downloads exactly that
commit; `npm run update-spec` (`fetch-spec.mjs --update [--ghes X.Y] [--sha S]`) re-pins to the
latest upstream commit/GHES version and regenerates the catalogues. We use the *bundled* variant
(one file, internal `$ref`s): ajv resolves the refs (including recursive ones) and it is a fraction
of the size of the dereferenced files. `--variant deref` is supported if ever needed.

## Validator: OpenAPI 3.0 → JSON Schema

`lib/spec.mjs` converts every `components/schemas` entry and inline schema to draft-07 for ajv
(`allErrors`, `ajv-formats` in full mode):

- `nullable: true` → `type: [T, "null"]` (and `null` added to `enum`); schemas without `type`
  are wrapped as `anyOf: [schema, {type: null}]`.
- `$ref` to `#/components/schemas/X` → a shared registered schema; `$ref` siblings are ignored as
  OAS 3.0 requires, except `nullable`, which is honoured.
- boolean `exclusiveMinimum/Maximum` → numeric draft-07 form.
- `discriminator`, `example(s)`, `readOnly`, `writeOnly`, `deprecated`, `xml`, `x-*` are dropped.
- `required`, `type`, `enum`, `format` (`date-time`, `uri`, `email`, `uri-template`, `date`,
  `uuid`, `hostname`, …) are enforced; GitHub-specific formats (`int64`, `repo.nwo`, `ssh-key`, …)
  are accepted as-is. `additionalProperties` follows the spec (allowed unless the spec says otherwise).

## Self-test against api.github.com (calibration)

`npm run selftest` runs the read-only scenarios (`--readonly`, default repo `cli/cli`): meta,
`/`, rate_limit, get repo, get user, list/get branch, git ref, list/get commit, git commit, tags,
README contents, list issues (+ pagination follow), get issue, issue comments, labels (+
pagination follow), milestones, list/get PR, PR files, list releases, latest release, release by
tag. It checks the remaining unauthenticated quota first (via `/rate_limit`, which is free) and
caps itself at 30 requests (`--max-requests`).

Result (2026-10-12, spec `58b1e0c00b`, 27 requests): **24/24 exercised operations conforming
(20 tier 1, 4 tier 2), 0 nonconforming**, against both the api.github.com and the GHES 3.22
descriptions (`reports/selftest-github.md`). The 27 recorded responses are committed as
`fixtures/github-cli-cli.recording.json`; `npm test` (`selftest-offline.mjs`) replays them against
both specs and also runs negative controls (dropped required field, int-as-string, null for a
non-nullable object, bad `date-time`/`uri`, nested `$ref` type error, enum violation, union with
no matching branch, object-for-array, undocumented status, 404 → not_implemented) to make sure the
validator rejects what it should.

Spec-vs-reality quirks tolerated:

1. **Cursor pagination has no `rel="last"`/`"first"`.** GitHub's `issues/list-for-repo` now
   paginates with `after=`/`before=` cursors and only sends `next`/`prev`. The oracle therefore
   requires `next` on page 1 and `prev` on page 2 but not `last`/`first`. (Offset-paginated lists
   such as labels still send all four; we should too.)
2. **`Link` URLs use `/repositories/{id}/…`**, not the `/repos/{owner}/{repo}/…` template of the
   operation; the oracle only requires the same origin and validates the followed page against the
   original operation.
3. **`oneOf` is relaxed to `anyOf` by default.** GitHub's unions (issue events, timeline items,
   contents, …) have overlapping branches and allow additional properties, so a valid object
   often matches several branches and fails "exactly one". This did not affect the 27 calibration
   responses (they also pass with `--strict-oneof`); it is a precaution for the event/timeline
   schemas the write scenarios hit.
4. **`nullable` next to `$ref`** is honoured even though OAS 3.0 says to ignore `$ref` siblings.

## Baseline: Gitea

`reports/baseline-gitea-1.24.md` is a run against a stock Gitea 1.24.6 (`/api/v1`, fresh admin
user, no `--org`/`--collaborator`): **31/1105 conforming (2.8%); tier 1 14/104 (13.5%);
tier 2 17/129 (13.2%)**, with 46 operations not implemented and 57 nonconforming. Its repo,
label and issue objects lack GitHub-required fields (`node_id`, `*_url` hypermedia, …); it has no
git data API, `/meta`, `/rate_limit` or search, and its contents API needs `sha` to create files,
which blocks the git/PR scenarios. Also use this run to check that the harness still works against
a server that is not GitHub. (These numbers come from validating Gitea against GitHub's spec. That
is the bar we have to clear. They do not measure Gitea's own API.)

Forgejo 16.0.6, run with the same flags as forge (`--org`, `--collaborator`): **29/1105 (2.6%);
tier 1 10/104 (9.6%); tier 2 19/129 (14.7%)** at `/api/v1`, and 0 at `/api/v3`. forge on the same
run: tier 1 102/104, tier 2 120/129. See [baseline.md](baseline.md) and
`tests/baseline/results/compat/`.

## Status and caveats

- The write scenarios have been dry-run end to end against Gitea, which tests the harness
  mechanics. They have **not** been run against github.com with a token, because we have no test
  account. When one is available, calibrate once with a throwaway account:
  `node run.mjs --base https://api.github.com --spec dotcom --token <PAT with repo, delete_repo,
  gist, user, admin:public_key scopes>`. Any scenario-side errors will show up as harness
  warnings or as unexpected statuses.
- Search results lag behind indexing, so search scenarios validate only the response shape.
- The run does not fail when a score is low. CI should compare `reports/latest.json` against a
  threshold.

## Layout

```
tools/conformance/
  fetch-spec.mjs          download + pin spec (spec.lock.json)
  catalogue.mjs           operation catalogue + priority.json validation -> catalogue.*.json
  priority.json           curated tier 1 / tier 2 operationIds
  run.mjs                 scenario runner + scorer
  selftest-offline.mjs    offline validator regression test (npm test)
  lib/spec.mjs            spec loading, operation index, OpenAPI->JSON Schema, ajv
  lib/oracle.mjs          HTTP client, per-response validation, pagination checks, verdicts
  lib/report.mjs          scoring, console + Markdown rendering
  scenarios/write.mjs     fixture-creating read/write scenarios
  scenarios/readonly.mjs  read-only scenarios (self-test)
  fixtures/               recorded api.github.com responses for the offline test
  reports/                latest.md, selftest-github.md, baseline-gitea-1.24.md (*.json gitignored)
  spec/                   downloaded descriptions (gitignored)
```

To add coverage, add calls to a scenario in `scenarios/write.mjs` using the operationId. Coverage
accounting picks up operationId string literals automatically.

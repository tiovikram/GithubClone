# forge

A GitHub-compatible code forge written in Rust, from scratch. Stock `git` and the stock `gh` CLI
work against it, its REST API follows GitHub's own OpenAPI description, and it runs on portable
parts only: Postgres, S3-compatible object storage (MinIO) and Kubernetes.

- **Server:** one binary, `forge` (`crates/forge`). It provides:
  - REST `/api/v3`
  - GraphQL `/api/graphql`
  - git smart HTTP and git over SSH
  - webhooks, with Postgres as the queue
  - the embedded web UI
- **Web UI:** a React SPA (`web/`), embedded into the binary at build time.
- **Design:** see [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).
  - Git packs are immutable objects in object storage.
  - A per-repo manifest row in Postgres is the atomic commit point.
  - Pods keep only a disposable cache, so any pod can serve any repo and any pod can be killed.

## Results

All numbers below were measured locally on this machine (Apple silicon, Docker Desktop
10 CPU / 8 GB). Every number can be reproduced with the commands in the next section.

| Oracle | Result | Details |
|---|---|---|
| Stock `git` over smart HTTP and SSH | clone, fetch, push, tags, shallow, protocol v2: all pass (17/17 smoke checks) | `crates/forge/tests/git_smart_http.rs`, `tests/baseline/git-smoke.sh` |
| `gh` CLI as an enterprise host | **166/166 forge-relevant gh commands (100%)**, 363/363 cases. That is 166/197 of *all* gh 2.102 commands; the other 31 need github.com-only services (Codespaces, Copilot, Sigstore) or are refused by gh for non-github.com hosts | [tests/gh/report.md](tests/gh/report.md), [docs/gh-coverage.md](docs/gh-coverage.md) |
| GitHub REST OpenAPI (GHES 3.22 spec) | **1105/1105 operations conforming (100%)**, 0 nonconforming, 0 not implemented. Tier 1 (what gh uses) 104/104, tier 2 (common octokit) 129/129 | [docs/conformance.md](docs/conformance.md) |
| vs Forgejo 16.0.6 (same cluster, same limits) | REST conformance 1105/1105 vs 45/1105; gh 166/166 vs 11/166 commands. API read saturation 6.7x Forgejo's throughput (1 replica each), 10.5x with 3 forge replicas. Git clone/fetch 1.9x (3.7x with 3 replicas). Write mix p95 6.6 ms vs 38.8 ms. Zero outage on graceful pod kills vs 4.6 s for Forgejo | [docs/baseline.md](docs/baseline.md) |
| Portability | Same image config digests (forge and forge-runner) and the same rendered manifests (sha256 `a88204a76d3f…`) on **k3d** and **kind**, with no code changes. `up.sh` about 55 s on either | [docs/deploy.md](docs/deploy.md) |
| In-cluster smoke | git push/clone; an `on: push` workflow run to `success` on an in-cluster runner pod (checkout, script step, artifact upload and download); `docker push`/`pull` against the OCI registry; `npm publish`/`install`. Passes on both clusters | `scripts/k8s/smoke-full.sh` |
| k6 load (HPA, 3–8 pods) | API reads 3.3k (k3d) and 3.7k (kind) req/s, p95 about 13 ms. Git clone/fetch about 900 req/s, clone p95 under 50 ms. Writes at 50 ops/s, p95 15 ms. 0% errors, all thresholds pass | [docs/deploy.md](docs/deploy.md) |
| Pod-kill chaos | 0 HTTP errors across about 477k requests with 24 forge pod kills (graceful and `--force`) on the two clusters. Pushes interrupted by kill -9 mid-push are atomic. 0 lost acknowledged pushes, `git fsck --strict` clean. Postgres and MinIO pod deletes recover in 3–4 s | [docs/deploy.md](docs/deploy.md) |
| Runner and registry chaos | Runner pod deleted gracefully mid-job: the job still finishes `success`. Force-deleted or `kill -9`: the run fails after the 3 min heartbeat timeout, re-running it succeeds, and no runs are left stuck. A 96 MB chunked OCI upload survives killing every forge pod at once: it resumes, and the sha256 matches | [docs/deploy.md](docs/deploy.md) |
| Security review | 24 issues fixed (including 2 critical git option injections), each with a regression test. Remaining accepted risks are listed | [docs/security.md](docs/security.md) |
| Tests | `cargo test`: 210 tests, all passing. Web UI e2e in headless Chrome: 32/32 flows (`web/e2e/run.mjs`) | |

Implemented, beyond the basics:
- Organizations, teams, collaborators, base permissions, custom repository and organization roles.
- Issues, labels, milestones, reactions and timelines.
- Pull requests:
  - reviews and review comments, including outdated detection;
  - merge, squash and rebase;
  - draft, cross-fork and auto-update branch.
- Branch protection and rulesets, both enforced on push and through the API (bypass lists and
  push bypass requests, commit signature rules).
- Releases with assets, gists, stars, watches, forks and templates.
- Search (repos, issues, code, commits, users).
- Contents and git-data APIs, commit statuses and the Checks API.
- Webhooks with deliveries.
- SSH keys, deploy keys and GPG keys.
- **Actions:** workflows run on `forge runner`, with shell or docker executors. It supports:
  - matrix, `needs` and outputs;
  - artifacts and caches;
  - composite actions;
  - environments with required reviewers;
  - sealed-box secrets and variables;
  - GITHUB_TOKEN permissions;
  - OIDC ID tokens;
  - runner groups at repo, org and enterprise level, with JIT runners.
- **GitHub Apps:** JWT auth, installations and scoped `ghs_` tokens.
- **Packages:** an OCI container registry (`/v2/`) and an npm registry.
- **Advanced security:**
  - secret scanning, with push protection;
  - code scanning, by SARIF upload;
  - Dependabot alerts, from OSV advisories against the dependency graph;
  - security advisories.
- **Enterprise admin:**
  - user suspension;
  - pre-receive hooks;
  - maintenance mode;
  - SCIM;
  - audit log;
  - site settings.
- Discussions and Projects v2.
- The OAuth device flow (`gh auth login`/`refresh`).

## Quick start (local, no Kubernetes)

```sh
scripts/install-deps.sh          # brew: rustup gh k3d kind helm k6 node postgresql@16 jq (records what it installed)
source scripts/env.sh            # PATH + DATABASE_URL
brew services start postgresql@16
psql postgres -c "CREATE ROLE forge LOGIN SUPERUSER PASSWORD 'forge'"   # once
createdb -O forge forge; createdb -O forge forge_test                  # once
(cd web && npm install && npm run build)   # the SPA is embedded into the binary
scripts/dev-server.sh            # builds and starts forge on http://localhost:3000 (log: data/server.log)
```

Open http://localhost:3000 and sign up. The first account becomes site admin. From the UI you
can create repos, browse code, open issues and pull requests, review, merge, publish releases,
manage SSH keys and tokens, and toggle dark mode. Tokens are created under *Settings → Tokens*,
or from the command line:

```sh
target/debug/forge admin create-token --login <you>
git clone http://<you>:<token>@localhost:3000/<you>/<repo>.git
```

**Using gh:** gh only speaks plain HTTP to github.localhost hosts, so `tests/gh/run.sh` connects
it through `http_unix_socket` (`FORGE_UNIX_SOCKET`). See the header of that script and
[docs/research/gh-protocol.md](docs/research/gh-protocol.md). For real use, put forge behind TLS
and run `gh auth login --hostname <host>`.

**Git over SSH:** set `FORGE_SSH_LISTEN=0.0.0.0:2222` and add your key in the UI.

## Running the oracles

```sh
cargo test -p forge                                     # unit + integration tests (uses DATABASE_URL_TEST)
tests/gh/run.sh                                         # gh suite; writes tests/gh/report.md
(cd tools/conformance && node run.mjs --base http://localhost:3000/api/v3 --token <token> \
    --org <org you own> --collaborator <second user> --collaborator-token <their token>)   # REST conformance
(cd web && caffeinate -dims node e2e/run.mjs --base http://localhost:3000 --login e2e1 --password 'e2e-pass-123')   # web UI e2e
scripts/k8s/e2e.sh k3d && scripts/k8s/e2e.sh kind       # cluster up, full smoke, k6 load, pod/runner/registry chaos, cluster down
tests/baseline/compat.sh; tests/baseline/k8s.sh; tests/baseline/bench.sh; tests/baseline/chaos.sh   # Forgejo comparison
```

Long runs should be wrapped in `caffeinate -dims` on macOS. If the Mac idle-sleeps mid-run,
every process pauses for about 15 minutes, and timeouts fire spuriously on wake.

## Uninstall

```sh
scripts/k8s/down.sh k3d; scripts/k8s/down.sh kind   # if clusters are still up
scripts/uninstall-deps.sh            # lists what it will remove and asks first (--yes to skip prompts)
```

`uninstall-deps.sh` first lists everything, then asks before each group. It removes:
- forge's k3d/kind clusters and Docker images, plus the base images pulled for the experiment;
- the Docker build cache;
- the Homebrew Postgres service and its data directory;
- repo-local build outputs (`target/`, `node_modules/`, `web/dist`, `data/`);
- the rust toolchain;
- the Homebrew formulae that `install-deps.sh` recorded in `scripts/.installed-by-forge`.

Formulae that were already installed are kept. It never touches Docker Desktop, your git
config or keychain, or `~/.config/gh`.

## Known gaps

Every operation in the GHES 3.22 REST spec is implemented and conforms to its schema. Where
the behaviour behind an operation is shallower than GitHub's, it is listed here:
- **Actions:**
  - No reusable workflows (`uses: ./.github/workflows/…`), `services:` containers or `concurrency:`.
  - Only composite actions run. JavaScript and Docker actions do not; `actions/checkout`,
    `upload-artifact`, `download-artifact` and `cache` are built in.
  - A runner killed with `kill -9` fails its job only after the 3 min heartbeat timeout.
  - Hosted labels such as `ubuntu-latest` map to whatever runner you register.
- **Advanced security:**
  - No Dependabot version or security update pull requests.
  - No CodeQL execution; code scanning accepts uploaded SARIF only.
- **Signatures:** commit signature verification needs a `gpg` binary on the server.
- **Rulesets:** not all rules are enforced yet. The open ones are code-owner review, last-push
  approval and required review thread resolution.
- **Packages:** no maven, rubygems, nuget or docker-legacy registries.
- **Security and permissions:** the full list is in
  [docs/security.md](docs/security.md#known-gaps--accepted-risks-not-fixed). The main ones:
  - classic tokens carry all scopes;
  - app tokens get read-only GraphQL;
  - emails are not verified;
  - a slow pusher holds the repo write lock.
- **Writes:** writes are about 2x slower than on 2026-10-12 (p50 3.0 → 5.6 ms at 50 it/s), while
  reads and git are unchanged. This is not investigated yet; see [docs/baseline.md](docs/baseline.md).
- **High availability:** the design is single-region. Postgres and MinIO run as single-instance
  StatefulSets in the manifests. Production would use their HA operators; forge itself needs no
  changes for that.

## Layout

```
crates/forge/        the server (src/, migrations/, tests/)
web/                 the SPA (src/, e2e/, mock/)
tools/conformance/   OpenAPI conformance harness
tests/gh/            gh command suite and catalogue
tests/load, tests/chaos, tests/baseline   k6, pod-kill, Forgejo comparison
deploy/              kustomize base + k3d/kind overlays
docs/                architecture, deploy, conformance, gh coverage, baseline, security
scripts/             install/uninstall, env, dev server, k8s up/down/e2e
```

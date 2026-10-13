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
| `gh` CLI as an enterprise host | **166/166 forge-relevant gh commands (100%)**, 349/349 cases. That is 166/197 of *all* gh 2.102 commands; the other 31 need github.com-only services (Codespaces, Copilot, Sigstore) or are refused by gh for non-github.com hosts | [tests/gh/report.md](tests/gh/report.md), [docs/gh-coverage.md](docs/gh-coverage.md) |
| GitHub REST OpenAPI (GHES 3.22 spec) | **tier 1 (what gh uses): 102/104 = 98.1%**, **tier 2 (common octokit): 120/129 = 93.0%**, all 1105 operations: 224 conforming (20.3%), **0 nonconforming** among those implemented | [docs/conformance.md](docs/conformance.md) |
| vs Forgejo 16.0.6 (same cluster, same limits) | Compatibility: 98% vs 10% (tier 1), gh 88/88 vs 0/88. API reads 7.0x the throughput (1 replica each), 10.6x with 3 forge replicas. Git clone/fetch 1.9x. Zero outage on graceful pod kills vs 4.1 s for Forgejo | [docs/baseline.md](docs/baseline.md) |
| Portability | Same image digest and same rendered manifests on **k3d** (k3s 1.35) and **kind** (k8s 1.37), with no code changes | [docs/deploy.md](docs/deploy.md) |
| k6 load (3 pods) | API reads about 5.2k (k3d) and 5.5k (kind) req/s, p95 about 11 ms, 0% errors. Git clone/fetch about 400 req/s. Writes at 50 ops/s, p95 under 7 ms | [docs/deploy.md](docs/deploy.md) |
| Pod-kill chaos | 0 HTTP errors across about 280k requests with 24 pod kills (graceful and `--force`). Pushes interrupted by kill -9 mid-push are atomic. 0 lost acknowledged pushes, `git fsck --strict` clean. Postgres and MinIO pod deletes recover in 3–4 s | [docs/deploy.md](docs/deploy.md) |
| Security review | 17 issues fixed (including 2 critical git option injections), each with a regression test. Remaining accepted risks are listed | [docs/security.md](docs/security.md) |
| Tests | `cargo test`: 107 tests, all passing. Web UI e2e in headless Chrome: 32/32 flows (`web/e2e/run.mjs`) | |

Implemented, beyond the basics:
- Organizations, teams and collaborators.
- Issues, labels, milestones, reactions and timelines.
- Pull requests:
  - reviews and review comments, including outdated detection;
  - merge, squash and rebase;
  - draft, cross-fork and auto-update branch.
- Branch protection (enforced) and rulesets.
- Releases with assets, gists, stars, watches, forks and templates.
- Search (repos, issues, code, commits, users).
- Contents and git-data APIs, commit statuses and checks.
- Webhooks with deliveries.
- SSH keys and deploy keys.
- Actions metadata (workflows, runs, secrets, variables).
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
(cd tools/conformance && node run.mjs --base http://localhost:3000/api/v3 \
    --token <token> --org <org you own> --collaborator <second user>)   # REST conformance
(cd web && caffeinate -dims node e2e/run.mjs --base http://localhost:3000 --login e2e1 --password 'e2e-pass-123')   # web UI e2e
scripts/k8s/e2e.sh k3d && scripts/k8s/e2e.sh kind       # cluster up, k6 load, pod-kill chaos, cluster down
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

- REST: about 80% of GitHub's 1105 operations are not implemented. These are mostly areas
  outside gh's and common octokit usage: Actions runners and artifacts, Packages, Copilot,
  billing, enterprise admin, code scanning and Dependabot. The two tier-2 operations missing
  are reactions on PR review comments.
- Actions: workflows, runs, secrets and variables exist as API metadata. There is no CI runner.
- Security and permission deviations are listed in
  [docs/security.md](docs/security.md#known-gaps--accepted-risks-not-fixed). The main ones:
  - every token has all scopes;
  - org members get write access by default;
  - emails are not verified;
  - the per-pod login throttle;
  - a slow pusher holds the repo write lock.
- Single-region design. Postgres and MinIO run as single-instance StatefulSets in the manifests.
  Production would use their HA operators; forge itself needs no changes for that.

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

# Baseline: forge vs Forgejo

Oracle 4 in [ARCHITECTURE.md](ARCHITECTURE.md) is "beat Forgejo/Gitea on GitHub-API compatibility
and on k6 performance". This document records a reproducible comparison against the latest
stable Forgejo. Both apps ran on the same machine, on the same cluster type, with the same pod
limits and with the same harnesses. Everything needed to rerun it is in `tests/baseline/`.
Raw outputs are in `tests/baseline/results/`.

## Summary

| | **Forgejo 16.0.6** (1 replica) | **forge** (1 replica) | **forge** (3 replicas) |
|---|---:|---:|---:|
| **REST conformance**, GitHub GHES 3.22 spec, tier 1 (gh core, 104 ops) | 10/104 (9.6%) at `/api/v1`; 0/104 at `/api/v3` | **102/104 (98.1%)** | (same binary) |
| REST conformance, tier 2 (octokit common, 129 ops) | 19/129 (14.7%) | **120/129 (93.0%)** | |
| REST conformance, all 1105 ops (conforming / of tested) | 29 (2.6%) / 20% of 145 tested | **224 (20.3%) / 99.1% of 226 tested** | |
| **gh CLI suite** (`tests/gh/run.sh`, gh as enterprise host) | 0/88 | **88/88** | |
| stock git smart HTTP (clone, fetch, push, tags, shallow, v2) | 17/17 | 17/17 | |
| **API reads, saturation** (64 VUs closed loop), throughput | 273 req/s | **1,900 req/s (7.0x)** | **2,894 req/s (10.6x)** |
| API reads, saturation, p50 / p95 / p99 | 75 / 951 / 1,046 ms | 29 / 71 / 82 ms | 19 / 50 / 64 ms |
| API reads @ 100 req/s, p50 / p95 / p99 | 5.5 / 18.4 / 29.1 ms | 3.9 / 5.1 / 5.8 ms | 3.8 / 5.8 / 6.5 ms |
| API reads @ 500 req/s | **not sustained**: 258 req/s done, 14.3k dropped, p95 6.3 s | 500 req/s, p95 2.7 ms | 500 req/s, p95 3.6 ms |
| API reads @ 1000 req/s | **not sustained**: 258 req/s done, 44.3k dropped | 1,000 req/s, p95 3.1 ms | 1,000 req/s, p95 4.2 ms |
| git clone (protocol v2), 1 VU: full clone p50 / p95 | 21 / 22 ms | 12 / 13 ms | 12 / 13 ms |
| git clone/fetch, 32 VUs: clones+fetches/s | 132 | 249 (1.9x) | 485 (3.7x) |
| git clone/fetch, 32 VUs: full clone p50 / p95 | 240 / 303 ms | 122 / 182 ms | 64 / 96 ms |
| write mix (create issue + comment) @ 50 it/s, p50 / p95 / p99 | 10.1 / 40.5 / 59.6 ms | 3.0 / 4.2 / 4.5 ms | 3.1 / 4.4 / 5.0 ms |
| **chaos A1**, graceful pod delete x6: longest / total outage | 4.1 s / 11.5 s | **0 s / 0 s** | **0 s / 0 s** |
| chaos A1: git loop pushes failed / clones failed | 50 / 50 of 122 | 0 / 0 of 100 | 0 / 0 of 95 |
| **chaos A2**, `--force --grace-period=0` x6: longest / total outage | 3.0 s / 4.0 s | 3.0 s / 3.5 s | **0 s / 0 s** |
| chaos A2: git loop pushes failed / clones failed | 6 / 4 of 82 | 5 / 3 of 92 | 0 / 0 of 96 |
| chaos: acknowledged pushes lost, `fsck --strict` failures | 0, 0 | 0, 0 | 0, 0 |

Performance figures are medians of 3 runs of 60 s each. Every scenario was run 3 times,
interleaved. Errors were 0.00% in every performance run on all three configurations.

**Verdict:** forge beats Forgejo on every axis measured here. On REST compatibility with GitHub's
own spec it scores 98% of tier 1 to Forgejo's 10%. Under gh, forge passes 88/88 and Forgejo 0/88;
gh cannot even authenticate against it. At one replica each with identical limits, forge sustains
7x the API read throughput and 1.9x the git clone/fetch throughput, with lower latency at every
fixed rate. Under pod kills, forge loses no availability on graceful deletes even at one replica.
forge also scales horizontally, which Forgejo cannot do on RWO storage. Stock git works the same
on both.

## What was compared

| | forge | Forgejo |
|---|---|---|
| version | `main` at `15977d4` + the Dockerfile fix below, image `forge:dev` built with the repo's Dockerfile (`sha256:4dba300d8a34…`) | `codeberg.org/forgejo/forgejo:16.0.6` (latest stable on 2026-10-12, image `sha256:e2c34a699543…`, Go 1.26.9, `+gitea-1.22.0`) |
| manifests | `tests/baseline/forge/`: `deploy/overlays/k3d` minus HPA and gc CronJob, `FORGE_BASE_URL` on :8088 | `tests/baseline/forgejo/`: Deployment (Recreate) + RWO PVC + Postgres |
| app pod requests / limits | 250m / 256Mi, **2 CPU / 1 GiB** | 250m / 256Mi, **2 CPU / 1 GiB** |
| replicas | 1 and 3 (fixed, no HPA) | 1 (RWO volume, see below) |
| Postgres | `postgres:16` (16.15), 500m/512Mi request, **4 CPU / 1.5 GiB** limit, same args (`max_connections=300`, `shared_buffers=256MB`, keepalives) | identical manifest (`tests/baseline/forgejo/postgres.yaml` differs only in DB and secret name) |
| DB pool | `FORGE_DB_POOL=20` | `MAX_OPEN_CONNS=20`, `MAX_IDLE_CONNS=20` (default idle is 2) |
| other deps | MinIO (`cgr.dev/chainguard/minio`, 2 CPU / 1 GiB limit) holds the packs | none; repos, queues and indexes on the PVC |
| probes, shutdown | readiness `/readyz`, preStop `sleep 5`, 60 s grace | readiness `/api/healthz`, preStop `sleep 5`, 60 s grace |
| logging | `forge=info,tower_http=warn` (no per-request lines) | `[log] LEVEL=Warn` (no router lines; slow-request warnings only) |
| exposure | NodePort 30080 ← host :8088 via k3d `-p 8088:30080@server:0` | same |

Both runtimes size their worker pools from the cgroup CPU quota. Go 1.25+ sets GOMAXPROCS from
the cgroup limit, and Rust's `available_parallelism` used by tokio honours cgroup v2 quotas.
With a 2-CPU limit, both run 2 workers.

**Forgejo settings used** (env `FORGEJO__section__KEY`, see
`tests/baseline/forgejo/kustomization.yaml`):
- `[database]` postgres, pool 20/20.
- `[cache] ADAPTER=memory` (default, in-process).
- `[queue] TYPE=level` (default, LevelDB on the PVC).
- `[indexer] ISSUE_INDEXER_TYPE=bleve` (default, on the PVC); `REPO_INDEXER_ENABLED=false`
  (default).
- `[session]` default (memory).
- `[server] DISABLE_SSH=true`, `LFS_START_SERVER=false`, `ROOT_URL=http://localhost:8088/`.
- `[security] INSTALL_LOCK=true`.
- `[service] DISABLE_REGISTRATION=true`.
- `[cron.update_checker] ENABLED=false`.
- `[log] LEVEL=Warn`.

No Redis or Memcached and no external indexer were used. This is the stock single-node setup,
and the only one possible with one RWO volume. Forgejo has no API rate limiter enabled by
default, and none was configured for either app.

**Why Forgejo runs 1 replica.** Forgejo keeps git repositories, LevelDB queues and the bleve
issue index on local disk. On a ReadWriteOnce volume only one pod can own them. Scaling out needs
RWX shared storage plus external queue, cache and indexer services (Redis, Meilisearch or
Elasticsearch), which is a different deployment. The fair comparison is therefore **1 vs 1** at
identical limits. forge at 3 replicas is reported to show horizontal scaling: 3x the app CPU,
same Postgres.

## Hardware and environment

- Apple M4 (10 cores), 24 GB RAM, macOS 26.6.2.
- Docker Desktop 29.8.0 with 10 CPUs / 7.75 GiB for its Linux VM (kernel 7.0.12-linuxkit).
- k3d v5.9.0 / k3s v1.35.5+k3s1 (containerd 2.2.3): 1 server + 2 agents, traefik disabled. This
  is the same topology as `scripts/k8s/up.sh`, under its own cluster name `bench` and host port
  8088.
- k6 v2.3.0 on the macOS host. Stock git 2.50.1 (Apple) and gh 2.102.0.
- Only one cluster existed at a time. The forge cluster was deleted and a fresh one created for
  Forgejo, and the images were imported (`docker save` + `k3d image import`), never pulled by the
  cluster.

## Methodology

### 1. GitHub-API compatibility (`tests/baseline/compat.sh`)

This runs on a docker-compose test bed (`tests/baseline/compat/docker-compose.yml`): one
Postgres 16 with two databases, `forge:dev` and Forgejo 16.0.6. Both get the same users
(`conf`, `collab`, admins) and an org `conforg` owned by `conf`, created through each server's own
admin CLI and org API.

- **REST:** `tools/conformance/run.mjs` uses the same harness, the same pinned spec (GHES 3.22,
  `58b1e0c00b`) and the same flags (`--org conforg --collaborator collab`) against:
  - forge `/api/v3`;
  - Forgejo `/api/v1`, its GitHub-like API;
  - Forgejo `/api/v3`, which is what any GitHub client pointed at the host gets.

  Reports: `tests/baseline/results/compat/conformance-*.{md,json,txt}`.
- **gh:** the unmodified case list of `tests/gh/run.sh` runs in its new `GH_SUITE_EXTERNAL=1`
  mode against both servers as GitHub Enterprise hosts (`forge.localhost`, `forgejo.localhost`).
  gh speaks plain HTTP over `http_unix_socket` through `tests/baseline/sockproxy.py` (a byte
  forwarder to each server's TCP port), so both are reached over the same transport. git reaches
  them through `url.insteadOf` and a static credential helper. The suite uses an isolated
  `GH_CONFIG_DIR`, `GIT_CONFIG_GLOBAL` and XDG dirs, with `GIT_CONFIG_NOSYSTEM=1`; the keychain
  is never touched. Reports: `results/compat/gh-{forge,forgejo}.{md,txt}`.
- **git:** `tests/baseline/git-smoke.sh` runs 17 checks with stock git over smart HTTP:
  - create a repo through each app's API, push, `ls-remote`;
  - clone, `fsck --strict`, a second clone;
  - push from clone A, fetch in clone B, compare;
  - push and delete a branch, push a tag;
  - check that a non-fast-forward push is rejected;
  - shallow clone;
  - check that protocol v2 is advertised.

  Reports: `results/compat/git-*.txt`.

### 2. Performance (`tests/baseline/k8s.sh`, `fixtures.sh`, `bench.sh`)

**Fixtures** (`fixtures.sh`) are identical on both apps and are created through each app's own
API plus stock git:
- `loadfix-1..3`: 120 commits over 60 files plus a README. The content and commit dates are
  deterministic, so the commit SHAs are identical on both apps (`7b4e98e`, `fc6ed60`, `790108e`).
  Each repo also has 30 open issues with one comment each.
- `writefix-1..3` (auto-init) receive the write mix.

**Scenarios** (`bench.sh`). Each scenario runs for 60 s and is repeated 3 times, interleaved:
run 1 of every scenario, then run 2, then run 3. A 35 s warm-up comes first and is not recorded.
The table reports the median over runs.

| scenario | k6 script | load model |
|---|---|---|
| `read-r100/500/1000` | `k6/api-read.js` | open model, `constant-arrival-rate` 100/500/1000 req/s, at most 400 in-flight VUs |
| `read-sat` | `k6/api-read.js` | closed loop, 64 VUs (throughput = what the server sustains) |
| `git-c1/8/32` | `tests/load/git-fetch.js` (unchanged) | closed loop, 1/8/32 VUs, each replaying the protocol-v2 clone that stock git 2.50 does: info/refs, `ls-refs`, `fetch` (80% full clones, 20% incremental fetches) |
| `write-r10/25/50` | `k6/api-write.js` | open model, 10/25/50 iterations/s; each iteration creates an issue and comments on it (2 requests) |

The API read mix has equal weights over five semantically equivalent endpoints. Paths are
identical except for the prefix, and the page-size parameter is `per_page` on forge and `limit`
on Forgejo.

| op | forge | Forgejo |
|---|---|---|
| get repo | `GET /api/v3/repos/{o}/{r}` | `GET /api/v1/repos/{o}/{r}` |
| list issues | `GET /api/v3/repos/{o}/{r}/issues?state=open&per_page=30` | `GET /api/v1/repos/{o}/{r}/issues?state=open&limit=30` |
| get file contents | `GET /api/v3/repos/{o}/{r}/contents/README.md` | `GET /api/v1/repos/{o}/{r}/contents/README.md` |
| get user | `GET /api/v3/users/{u}` | `GET /api/v1/users/{u}` |
| list user repos | `GET /api/v3/user/repos?per_page=30` | `GET /api/v1/user/repos?limit=30` |

All requests use `Authorization: token <PAT>`. Git uses HTTP Basic `x-access-token:<PAT>`, which
both accept. Write request bodies are byte-identical.

Per run, `bench.sh` also records `kubectl top pods` and the host load average about 40 s in
(`*.top`), along with pod and image metadata (`meta.txt`).

### 3. Chaos parity (`tests/baseline/chaos.sh`)

These are phases A1 and A2 of `tests/chaos/pod-kill.sh`, applied the same way to either app:
- 6 kills, one every 15 s, of a random app pod:
  - A1: `kubectl delete pod`;
  - A2: `kubectl delete pod --grace-period=0 --force`.
- Load during the kills:
  - k6 read mix with 10 VUs plus `git-fetch.js` with 4 VUs, both with a 10 s request timeout;
  - `tests/chaos/git-integrity-loop.sh`, unchanged: commit, push, fresh clone, `fsck --strict`,
    and check that every acknowledged push is present.
- Added: a 0.5 s availability probe (`GET /users/<owner>`) that measures the outage window.

Reports: `tests/baseline/results/chaos-{forgejo-1,forge-1,forge-3}.md`, with raw k6 output, probe
and git logs under `results/raw/chaos-*`.

## Results

### Performance

<!-- generated: node tests/baseline/summarize.mjs  (also written to tests/baseline/results/summary.md) -->

Errors were 0.00% in every scenario for all three configurations, so the error rows are omitted
here; they are in `results/summary.md`. *Dropped* means iterations k6 could not start because all
400 in-flight VUs were waiting, which is the open-model sign that the server cannot keep up with
the offered rate.

| scenario | metric | Forgejo, 1 replica | forge, 1 replica | forge, 3 replicas |
|---|---|---:|---:|---:|
| API reads @ 100 req/s | p50 / p95 / p99 ms | 5.5 / 18.4 / 29.1 | 3.9 / 5.1 / 5.8 | 3.8 / 5.8 / 6.5 |
| API reads @ 500 req/s | achieved req/s | **258** (14,291 dropped) | 500 | 500 |
| | p50 / p95 / p99 ms | 330 / 6,310 / 6,787 | 2.4 / 2.7 / 3.0 | 2.5 / 3.6 / 3.8 |
| API reads @ 1000 req/s | achieved req/s | **258** (44,301 dropped) | 1,000 | 1,000 |
| | p50 / p95 / p99 ms | 345 / 6,348 / 6,842 | 2.5 / 3.1 / 3.6 | 2.8 / 4.2 / 4.8 |
| API reads, saturation (64 VUs) | req/s | 273 | 1,900 | 2,894 |
| | p50 / p95 / p99 ms | 75 / 951 / 1,046 | 29.1 / 70.6 / 81.5 | 19.1 / 50.4 / 64.2 |
| git clone/fetch, 1 VU | clones+fetches/s (HTTP req/s) | 51.7 (155) | 94.4 (283) | 91.7 (275) |
| | full clone p50 / p95 / p99 ms | 21 / 22 / 24 | 12 / 13 / 14 | 12 / 13 / 14 |
| git clone/fetch, 8 VUs | clones+fetches/s (HTTP req/s) | 134 (403) | 225 (676) | 380 (1,140) |
| | full clone p50 / p95 / p99 ms | 59 / 86 / 96 | 34 / 57 / 64 | 22 / 27 / 30 |
| git clone/fetch, 32 VUs | clones+fetches/s (HTTP req/s) | 132 (396) | 249 (747) | 485 (1,454) |
| | full clone p50 / p95 / p99 ms | 240 / 303 / 328 | 122 / 182 / 200 | 64 / 96 / 113 |
| write mix @ 10 it/s (20 req/s) | p50 / p95 / p99 ms | 12.9 / 26.8 / 37.3 | 7.2 / 9.4 / 10.8 | 7.4 / 10.1 / 12.3 |
| write mix @ 25 it/s (50 req/s) | p50 / p95 / p99 ms | 9.5 / 30.1 / 46.0 | 4.2 / 7.2 / 7.9 | 4.3 / 6.9 / 7.8 |
| write mix @ 50 it/s (100 req/s) | p50 / p95 / p99 ms | 10.1 / 40.5 / 59.6 | 3.0 / 4.2 / 4.5 | 3.1 / 4.4 / 5.0 |

Per-endpoint p95 at saturation (ms):

| endpoint | Forgejo, 1 replica | forge, 1 replica | forge, 3 replicas |
|---|---:|---:|---:|
| get repo | 79.5 | 33.4 | 19.2 |
| list issues (30) | 1,046 | 63.3 | 42.2 |
| get contents | 109 | 41.4 | 34.7 |
| get user | 38.9 | 29.2 | 15.8 |
| list user repos | 208 | 81.4 | 63.9 |

At low load (100 req/s) Forgejo's list-issues p50 is 17 ms and get-contents 10 ms, against 2 to
6 ms for forge.

Where the CPU went, from `kubectl top` during saturation:

| | app pods | Postgres |
|---|---|---|
| Forgejo, read-sat | 1.6 to 1.7 CPU (of 2) | 0.4 to 0.5 CPU |
| forge 1x, read-sat | 2.0 CPU (at the limit) | 1.8 to 2.0 CPU |
| forge 3x, read-sat | 3 x 1.2 to 1.3 CPU | **3.0 CPU** (of 4) |
| forge 1x, git-c32 | 2.0 CPU | 0.5 CPU |

Forgejo is bound by its own CPU. forge at one replica is bound by its 2-CPU limit, and at 3
replicas it is bound by Postgres plus the shared host (see the findings below).

### Chaos (6 kills per phase, during load)

| | Forgejo, 1 replica | forge, 1 replica | forge, 3 replicas |
|---|---|---|---|
| A1 graceful: replacement ready after | 9 to 10 s | 3 to 6 s | 3 to 6 s |
| A1: availability probe, longest / total outage | 4.1 s / 11.5 s | 0 / 0 | 0 / 0 |
| A1: k6 failed requests | 43,694 / 66,045 (66%)* | 2 / 114,804 | 0 / 164,538 |
| A1: git loop pushes ok/failed, clones ok/failed | 72/50, 72/50 | 100/0, 100/0 | 95/0, 95/0 |
| A2 force: replacement ready after | 4 to 5 s | 3 to 6 s | 3 to 6 s |
| A2: availability probe, longest / total outage | 3.0 s / 4.0 s | 3.0 s / 3.5 s | 0 / 0 |
| A2: k6 failed requests | 436 / 23,305 (1.9%) | 948 / 93,652 (1.0%) | 2 / 151,259 |
| A2: git loop pushes ok/failed, clones ok/failed | 76/6, 78/4 | 87/5, 89/3 | 96/0, 96/0 |
| acknowledged pushes missing later, `fsck` failures | 0, 0 | 0, 0 | 0, 0 |

\* **Read failed-request counts with care.** During an outage, the k3d load balancer (nginx,
`max_fails=1 fail_timeout=10s`) resets new connections at once, and the closed-loop VUs then spin
on instant failures. The count therefore measures how fast clients retry, not how long the
service was down. The probe's outage seconds and the git loop are the meaningful recovery
numbers.

What happens on a graceful delete, from a 0.5 s trace of one Forgejo kill
(`kubectl delete pod`):
- The old pod keeps serving during its 5 s preStop. kube-proxy falls back to terminating
  endpoints that are still serving.
- At SIGTERM (t+5.3 s) Forgejo stops serving and the service is down.
- The replacement pod had been Running since t+2.3 s, but it turned Ready only at t+8.9 s,
  after the old pod had exited (t+6.5 s).
- The result is a gap of about 3.6 s per kill. The likely cause is the shared RWO volume (the
  LevelDB queue and bleve index locks are held by the old pod); the trace does not prove it.

forge's replacement is Ready in about 3 s, inside the old pod's preStop window, so even a single
replica hands over with no gap.

A forced delete removes the endpoint at once, so a single replica of either app is down until
its replacement is Ready: 3 to 4 s for both. At 3 replicas forge has no outage at all.

Neither app lost an acknowledged push or produced a corrupt repository.

## Findings

**Bug fixed in forge (Dockerfile).** The runtime image was `debian:bookworm-slim`, which ships
git **2.39.5**. forge's rebase merge and its `rebaseable` check use `git merge-tree --write-tree
--merge-base=<c>` (`crates/forge/src/pulls/git.rs`, `merge_tree`/`rebase`), and that option only
exists in git ≥ 2.40. In the container, `gh pr merge --rebase` therefore failed with "This branch
can't be rebased", and the PR's `rebaseable` was false. The gh suite scored 86/88 against the
image even though it scores 88/88 against a natively built binary, because macOS git is 2.50.
The fix is a runtime base of `debian:trixie-slim` (git 2.47.3); the Rust build stage is
unchanged, and the bookworm-built binary runs on trixie's newer glibc. After the fix the image
scores 88/88. All forge numbers above are from the fixed image. Suggested follow-up, not done
here: have `forge serve` check `git --version` at startup and refuse to start below 2.40.

**forge findings (for the forge team):**
- **Postgres is forge's horizontal-scaling ceiling.** A read-mix request costs about 1 ms of
  Postgres CPU: Postgres used 1.9 CPU at 1,900 req/s and 3.0 CPU at 2,900 req/s. At 3 replicas
  the forge pods were only about 60% busy while Postgres approached its 4-CPU limit and the host
  was saturated, so the gain from 1 to 3 replicas was 1.5x for reads, against 1.9x for git
  (which uses Postgres much less). Fewer queries per request would raise this ceiling directly:
  caching the token-to-user lookup, and fetching the repo, owner and permission in one query.
  Forgejo, by contrast, used only 0.5 CPU of Postgres at its saturation point.
- forge at one replica is limited by its 2-CPU limit at 1,900 req/s (about 1 ms of app CPU per
  read).

**Harness findings:**
- The k3d server load balancer (nginx stream, `worker_connections 1024`) can hold only about
  512 proxied connections. A first Forgejo run with up to 600 in-flight k6 VUs produced 31 to 37%
  "EOF" failures that came from the load balancer, not Forgejo (`worker_connections are not
  enough` in the serverlb log). k6 in-flight VUs are therefore capped at 400 for every app
  (`k6/lib.js`). forge never exceeded 64 in-flight VUs in any run, so its numbers are
  unaffected. The discarded run's log is kept in
  `results/raw/exploratory/forgejo-1-unbounded-vus.log`.
- Laptop sleep. The host slept 15 min in the middle of an early unattended run (pmset
  "Maintenance Sleep"), which stretched a 20 s k6 test to 196 s. The deploy.md k3d write anomaly
  ("k6 only exited about 4 minutes after the 60 s scenario") may well have the same cause.
  `bench.sh` and `chaos.sh` now re-exec themselves under `caffeinate -ims`.
- `tests/gh/run.sh` gained an additive `GH_SUITE_EXTERNAL=1` mode for any running server. In that
  mode it skips the SQL fallback for commit statuses, so it never writes into a database it does
  not own. With default settings the suite behaves exactly as before.

## Caveats

- **Single host.** Everything ran in one Docker Desktop VM (10 CPU / 7.75 GiB) on one laptop:
  both clusters (one at a time), Postgres, MinIO, the app pods, and k6 itself. k6, the git
  clients and the app compete for the same cores. The absolute numbers are a relative baseline,
  not capacity figures. Saturation at 3 replicas is partly host-bound, since the 10 host cores
  were shared by about 4 forge CPUs, 3 Postgres CPUs, k6 and other work.
- **Other work on the machine.** Other engineers' builds and tests ran on the same machine
  during these runs; the host load average is recorded in every `*.top` file and ranged from
  about 5 to 17. Interleaving runs and taking the median of 3 limits the effect. The first
  forge-1 run (`read-r100`/`read-r500` run 1 has p99 44 to 74 ms) is visibly noisier than runs
  2 and 3, and the median discards it.
- **Topology differences that cannot be removed.** forge needs MinIO, an extra pod with its own
  2-CPU limit that was idle during reads (1m CPU). Forgejo keeps repos on a local-path PVC
  instead. Forgejo's image starts as root and is not hardened (no read-only root filesystem).
  forge runs non-root with a read-only root.
- **Forgejo is tested through GitHub's spec.** The conformance numbers measure how GitHub-
  compatible Forgejo's API is, not how well Forgejo implements its own API. Its contents API
  requires `sha` to create a file, and it has no git refs API. Those two fixture steps block the
  PR, branch and commit scenarios, so 960 of 1105 operations went untested ("% of tested" is 20%
  over 145 tested operations). At `/api/v3`, every path returns 404 Not Found: the harness's
  first step, `GET /user`, gets 404, which blocks every scenario after it, and the 7 meta calls
  it can still make are all `not_implemented`. That is all a GitHub client pointed at the host
  ever sees. gh fails at
  `gh auth status` (no `/api/v3/user`) and at every GraphQL call (`/api/graphql` returns 405).
- **The comparison is at equal resources, not equal features.** Forgejo has a web UI, Actions
  and more that forge does not have, or that was not exercised here. The benchmark only uses
  endpoints both have.
- The issue-list scenario reads 30 issues of 30, with bodies. Forgejo's list issues is its
  slowest endpoint, so the read mix's equal weighting is part of why its saturation point is
  low. Per-endpoint numbers are given so other weightings can be estimated.
- forge's `fetch` and Forgejo's `fetch` send the same pack bytes for the same deterministic
  fixtures (identical commit SHAs), so the git comparison is like for like.

## Reproduce

```sh
# compatibility (docker compose, ~5 min)
tests/baseline/compat.sh                     # up conformance gh git down

# performance + chaos (k3d, one cluster at a time; ~45 min per configuration)
docker build --provenance=false --sbom=false -t forge:dev .
tests/baseline/k8s.sh up
tests/baseline/k8s.sh deploy forge 1 && tests/baseline/fixtures.sh data/bench/forge.env
RUNS=3 tests/baseline/bench.sh data/bench/forge.env forge-1
tests/baseline/chaos.sh data/bench/forge.env forge-1
tests/baseline/k8s.sh scale forge 3
RUNS=3 tests/baseline/bench.sh data/bench/forge.env forge-3
tests/baseline/chaos.sh data/bench/forge.env forge-3
tests/baseline/k8s.sh down
tests/baseline/k8s.sh up
tests/baseline/k8s.sh deploy forgejo && tests/baseline/fixtures.sh data/bench/forgejo.env
RUNS=3 tests/baseline/bench.sh data/bench/forgejo.env forgejo-1
tests/baseline/chaos.sh data/bench/forgejo.env forgejo-1
tests/baseline/k8s.sh down
node tests/baseline/summarize.mjs            # -> tests/baseline/results/summary.md
```

Layout:

```
tests/baseline/
  compat.sh, compat/docker-compose.yml, compat/initdb.sql   compat test bed + runner
  git-smoke.sh, sockproxy.py                                stock-git checks; unix-socket forwarder for gh
  k8s.sh                                                    k3d "bench" cluster, deploy/scale, admin token -> data/bench/<app>.env
  forge/kustomization.yaml                                  forge overlay (no HPA/gc, :8088)
  forgejo/{kustomization,forgejo,postgres,namespace}.yaml  Forgejo + Postgres
  fixtures.sh, bench.sh, chaos.sh, summarize.mjs
  k6/{lib,api-read,api-write}.js                            (git uses tests/load/git-fetch.js unchanged)
  results/summary.md, results/chaos-*.md, results/compat/*, results/raw/<label>/<scenario>-run<N>.{json,txt,top}
```

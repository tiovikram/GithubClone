# Deploying forge on Kubernetes (k3d, kind, any cluster)

forge runs on Postgres, S3-compatible object storage (MinIO) and Kubernetes, and nothing else.
k3d and kind use the **same image and the same rendered manifests**. Only the cluster-creation
block in `scripts/k8s/up.sh` differs. Everything runs locally: no registry, no cloud, no accounts.

## Layout

```
Dockerfile, .dockerignore     multi-stage: node (web/dist) -> rust (cargo build --release) -> debian trixie-slim
                              + git; two targets: `forge` (server, default) and `forge-runner` (+ CLI tools)
deploy/base/                  namespace forge; postgres + minio StatefulSets, bucket Job, forge Deployment,
                              NodePort Service, PDB, HPA, gc CronJob, forge-runner Deployment (Actions),
                              ConfigMap + Secret generators
deploy/overlays/{k3d,kind}/   both are just `resources: [../../base]` (identical on purpose)
scripts/k8s/up.sh <flavor>    create cluster, build both images, import, apply -k, wait, admin user + token, smoke test
scripts/k8s/smoke-full.sh     git push/clone, an `on: push` workflow on the in-cluster runner (bash, checkout,
                              upload-artifact) + artifact download, docker push/pull, npm publish/install
scripts/k8s/proof.sh <flavor> portability evidence: manifest hash, image config digests on the nodes
scripts/k8s/down.sh <flavor>  delete the cluster
scripts/k8s/e2e.sh <flavor>   up + smoke-full + k6 suite + the three chaos scripts + down
tests/load/                   k6: api-read.js, git-fetch.js, api-write.js; run.sh writes results/<cluster>-<date>.md
tests/chaos/pod-kill.sh       pod-kill chaos under load + git integrity checks; writes results/<flavor>-<date>.md
tests/chaos/runner-kill.sh    kill the runner pod running an Actions job; writes results/runner-<flavor>-<date>.md
tests/chaos/registry-kill.sh  kill forge pods during a chunked OCI blob upload; results/registry-<flavor>-<date>.md
scripts/uninstall-deps.sh     removes everything installed for the experiment (asks first; --yes to skip)
```

```
host :8080 ──(k3d -p / kind extraPortMappings)──> NodePort 30080 ──> Service forge ──> forge pods (3..8, HPA)
                                                                        │  emptyDir git cache only
                                         Postgres (StatefulSet, PVC) <──┤  refs/manifests, advisory locks
                                         MinIO    (StatefulSet, PVC) <──┘  immutable packs (s3://forge/data)
```

## How to run

```sh
scripts/k8s/up.sh k3d                  # ~1 min with warm images (SKIP_BUILD=1); a source change adds ~8 min
source data/k8s/k3d.env                # FORGE_URL, FORGE_TOKEN, KUBE_CONTEXT
scripts/k8s/smoke-full.sh k3d          # ~10 s: git, Actions on the in-cluster runner, docker push/pull, npm
tests/load/run.sh $FORGE_URL $FORGE_TOKEN          # DURATION=60s READ_VUS=20 GIT_VUS=10 WRITE_RATE=50
tests/chaos/pod-kill.sh k3d            # ~10 min; KILLS, INTERVAL, MIDPUSH_RUNS, PHASES, DEP_TARGETS
tests/chaos/runner-kill.sh k3d         # ~10 min; CASES="graceful force crash", JOB_SECS, KILL_AFTER
tests/chaos/registry-kill.sh k3d       # ~2 min; CASES, SIZE_MB, CHUNK_MB, RATE
scripts/k8s/proof.sh k3d               # manifest hash + image config digests (diff against kind)
scripts/k8s/down.sh k3d
# same commands with `kind`; or everything in one go:
scripts/k8s/e2e.sh k3d && scripts/k8s/e2e.sh kind
```

Run one cluster at a time. Both bind host port 8080, and Docker Desktop has 8 GB.

## Design decisions

- **Exposure: NodePort, not Ingress.** k3d ships traefik, kind ships no ingress controller, and
  clouds ship their own. An Ingress would need a controller-specific install and class on each
  cluster. A `type: NodePort` Service on 30080 behaves the same everywhere, and the
  cluster-creation step maps host 8080 to it (`k3d -p 8080:30080@server:0`, kind
  `extraPortMappings`). In production you put your LB or Ingress in front of the `forge`
  Service. traefik is disabled on k3d so the two clusters look the same.
- **Migrations: no Job.** `forge serve` runs migrations at startup under
  `pg_advisory_lock(7262620)`, so pods that start together are safe. A migration Job would add
  an immutable-Job lifecycle problem on re-apply and buy nothing.
- **Startup ordering:** an init container (the forge image's own bash with `/dev/tcp`, so there
  is no extra image) waits for Postgres and for the bucket. MinIO returns 404 until the `mc mb
  --ignore-existing` Job has run.
- **Graceful shutdown:** `preStop: sleep 5` lets endpoints and kube-proxy drop the pod, then
  SIGTERM (forwarded by tini) triggers axum's graceful drain. `terminationGracePeriodSeconds:
  60`, PDB `minAvailable: 2`, rolling update `maxUnavailable: 0`.
- **Spread and scale:** topology spread over hostname and zone (`ScheduleAnyway`, so it never
  blocks scheduling). HPA scales 3 to 8 at 70% CPU. Under the k6 suite it reached 8 replicas on
  both clusters. HPA needs metrics-server: k3s bundles it, and `up.sh` installs it on kind as a
  cluster add-on, not an app manifest change.
- **Postgres keepalives:** `tcp_keepalives_idle=20`, `interval=5`, `count=3` and
  `client_connection_check_interval=5s`. Without them, the session-level per-repo advisory lock
  of a pod that vanished without closing its socket would be held until the kernel's
  multi-hour keepalive expired.
- **Security:** forge runs as non-root uid 10001 with a read-only root filesystem, all
  capabilities dropped and the RuntimeDefault seccomp profile. Writable paths are only the
  emptyDirs `/var/cache/forge` (4 Gi) and `/tmp`.
- **Images:** MinIO stopped publishing images to Docker Hub and quay.io in 2025, and
  `minio/minio` no longer resolves. The manifests use `cgr.dev/chainguard/minio` and
  `minio-client`, which are free builds of upstream MinIO. Pin them by digest in production.
  Dependency images are pulled once on the host and imported with `docker save --platform
  linux/<arch>` + `k3d image import` / `kind load image-archive`. Plain `kind load docker-image`
  fails on Docker Desktop's containerd store with multi-arch indexes. The forge image is built
  with `--provenance=false --sbom=false` for the same reason.
- **Docker build.** One Dockerfile, two targets: `forge` (default: trixie-slim + git 2.47,
  ca-certificates, tini; 406 MB in the containerd store) and `forge-runner` (the same plus the
  runner tools; 464 MB). The build stage adds cmake, clang/libclang and perl for aws-lc-rs.
  The forge crate itself is compiled with one cargo job (`CARGO_BUILD_JOBS=1`, a build arg):
  with the default parallelism rustc grew past the ~7.8 GB of Docker Desktop's VM and was
  OOM-killed, even with LTO off. One jobserver token makes rustc optimise one codegen unit at a
  time; the profile and binary are unchanged, peak memory is ~6.8 GB and the crate takes ~9 min
  instead of ~4. Do not build while a cluster is running in the same 8 GB VM.
- **Credentials:** the base kustomization uses a `secretGenerator` with development values.
  Replace it with your secret manager in production; the key names stay the same.

The S3 client needed no fixes for MinIO. `build_object_store` already uses
`AmazonS3Builder::from_env()` with path-style requests, and `AWS_ENDPOINT_URL`,
`AWS_ALLOW_HTTP` and `AWS_REGION` come from the ConfigMap.

### Code change: TCP_NODELAY (crates/forge/src/main.rs)

The first k3d load run showed every streamed git response (`ls-refs`, `fetch`) taking about
42 ms at p50, even for tiny responses, while `info/refs` (a single buffered body) took 2 to 4 ms.
That 42 ms is Nagle's algorithm interacting with the client's delayed ACK: the response is
streamed from `git upload-pack` in small chunks. forge now sets `TCP_NODELAY` on accepted
connections (`axum::serve::ListenerExt::tap_io`).

| k3d, 10 VUs git-fetch, 60 s | before | after |
|---|---|---|
| requests/s | 325.7 | 400.9 |
| http p50 | 42.0 ms | 9.8 ms |
| ls-refs p95 | 44.3 ms | 20.1 ms |
| full clone sequence p95 | 106 ms | 90 ms |

The "before" runs are kept as `tests/{load,chaos}/results/k3d-*-pre-nodelay.md`.

## Portability proof (2026-10-18, `main` at `284dbe1` + this branch)

| | k3d | kind |
|---|---|---|
| cluster | k3s v1.35.5, 1 server + 2 agents | Kubernetes v1.37.0, 1 control-plane + 2 workers |
| host images (`docker image inspect`) | `forge:dev` `sha256:0959ac9da698…`, `forge-runner:dev` `sha256:68a2eeb8ff87…` | same two images (built once, `SKIP_BUILD=1`) |
| image config digest on the nodes (`scripts/k8s/proof.sh`) | forge `e4ceea76db89…`, runner `8db21f4ce27d…`, postgres `ed134ce8cc30…`, minio `4725fac3bcd8…`, mc `7e630b42bead…` | identical (all five) |
| rendered manifests sha256 (`kubectl kustomize deploy/overlays/<flavor>`) | `a88204a76d3f…` | `a88204a76d3f…` (same) |
| up.sh: cluster + import + apply + rollout (forge ×3, runner ×2) + smoke | 534 s incl. the ~8 min `-j1` Rust rebuild / 55 s with built images | 58 s |
| up.sh smoke: GET /user, POST /user/repos, git push, 3 fresh clones (HEAD match, fsck --strict) | pass | pass |
| `smoke-full.sh`: git push + 2 clones; `on: push` workflow (actions/checkout, bash step with git/tar/curl/jq, upload-artifact) **`success` on an in-cluster forge-runner pod**; artifact zip downloaded via REST, content checked; `docker login/push/pull localhost:8080/admin/smoke-img` (image id and file content identical after the round trip, `tags/list`); `npm publish` + `npm install` + `require()` of `@admin/smoke-pkg` | pass (7 s) | pass (5 s) |

The runtime imageID string differs per runtime (k3s reports the config digest; kind's
`load image-archive` reports `import-<date>@<manifest digest>`), so `proof.sh` compares the
config digest and layer `diff_ids` from `crictl inspecti` on a node. The pod-template hash
differs (6d58bb7498 vs 75d8b58fb4) only because the two API server versions apply different
defaults to the same manifests.

Registry over the NodePort: Docker Desktop's daemon reaches the cluster's `localhost:8080`
directly and allows plain HTTP to `localhost`, so no `insecure-registries` entry is needed.
`smoke-full.sh` runs docker with a throwaway `DOCKER_CONFIG` and npm with a throwaway
userconfig/cache, so no credential lands in the user's keychain or `~/.npmrc`.

## Load test results (2026-10-18; k6 v2.3.0, 60 s per script, Apple M4, Docker Desktop 10 CPU / 8 GB)

Mix: **api-read** = 20 closed-loop VUs over GET `/repos/{o}/{r}`, `/user`, `/user/repos`,
`/users/{u}`, `/users/{u}/repos`, `/languages`, `/rate_limit` ("core") plus issues, contents,
branches, commits and pulls ("optional"). **git-fetch** = 10 VUs, each replaying a
protocol-v2 clone: info/refs, `ls-refs` and `fetch` (bodies match a `GIT_TRACE_PACKET` capture
of git 2.50, see `tests/load/fixtures/clone-v2.trace`) against 3 fixture repos with 120 commits
each. **api-write** = 50 ops/s fixed arrival rate: create repo, PATCH repo, create issue +
comment.

| cluster | script | requests | req/s | errors | p50 | p95 | p99 | thresholds |
|---|---|---|---|---|---|---|---|---|
| k3d | api-read | 200,476 | 3,341 | 0 % | 4.9 ms | 13.4 ms | 18.9 ms | PASS |
| k3d | git-fetch | 57,120 (19,040 clones) | 952 | 0 % | 5.9 ms | 15.5 ms | 20.4 ms | PASS (clone p95 43 ms) |
| k3d | api-write | 4,796 (3,000 iterations) | 80 | 0 % | 5.7 ms | 14.8 ms | 17.7 ms | PASS |
| kind | api-read | 219,236 | 3,653 | 0 % | 4.4 ms | 12.6 ms | 17.7 ms | PASS |
| kind | git-fetch | 53,400 (17,800 clones) | 890 | 0 % | 6.5 ms | 17.7 ms | 22.0 ms | PASS (clone p95 46 ms) |
| kind | api-write | 4,808 (3,001 iterations) | 80 | 0 % | 5.5 ms | 15.0 ms | 17.0 ms | PASS |

The HPA reached 8 replicas on both clusters. Thresholds: core read p95 < 250 ms and
p99 < 600 ms; info/refs and ls-refs p95 < 300 ms; fetch p95 < 1.5 s; clone p95 < 2 s;
create-repo p95 < 800 ms; error rate < 1 %; checks > 99 %. These runs share one laptop with k6
itself, so the numbers are a relative baseline, not a capacity figure.

**Compared with the previous run (2026-10-12, `main` before Checks/orgs/Actions/packages):**
api-read fell from 5,236 / 5,483 req/s to 3,341 / 3,653 req/s. This is the workload changing,
not a regression: the "optional" endpoints (issues, contents, branches, commits, pulls) answered
404 on the old main (optional p50 0.73 ms) and now do real work (optional p50 7.3 ms, much of it
git-backed contents/commits). Core endpoints went from p50 2.8 ms to 3.8 ms. git-fetch more than
doubled (401 / 422 -> 952 / 890 req/s, clone p95 90 / 133 -> 43 / 46 ms). api-write now creates
the issue and comment for real (1.6 requests per iteration instead of 1.0). The old k3d
api-write anomaly (k6 exiting 4 min late) did not recur. The fixed-limit comparison against
Forgejo in [baseline.md](baseline.md) measures the per-request cost directly.

Found and fixed by this run: after api-write had created ~600 repos for the admin user, a
30-repo page of `GET /user/repos` or `/users/{u}/repos` took ~35 ms, because the list query
called the non-inlinable `forge_repo_readable()` for every candidate row (page and count). The
list now tests the two cases that function accepts first (public, owned by the viewer) inline
(`crates/forge/src/api/repos.rs`, `list_rows`): page query 14.7 ms -> 0.26 ms at 632 repos.

Full reports: `tests/load/results/{k3d-20261018-003509,kind-20261018-005944}.md`. Previous:
`{k3d-20261012-174518,kind-20261012-180502}.md`.

## Chaos results

### Pod kills under load (`tests/chaos/pod-kill.sh`)

Phases A1 and A2 each run for 110 s: k6 api-read (10 VUs) and git-fetch (4 VUs) with a 10 s
request timeout, plus a loop that commits ~40 KB, pushes, fresh-clones, runs
`git fsck --strict`, and checks that every acknowledged push is an ancestor of `origin/main`.
Six random forge pods are killed, one every 15 s.

| | k3d | kind |
|---|---|---|
| A1: graceful `kubectl delete pod` ×6: HTTP errors | **0 / 116,317** (0.000 %, limit 0.5 %) | **0 / 119,168** (0.000 %) |
| A1: git loop pushes ok/failed, clones ok/failed | 85/0, 85/0 | 83/0, 83/0 |
| A2: `--grace-period=0 --force` ×6: HTTP errors | **0 / 125,989** (0.000 %, limit 5 %) | **0 / 115,915** (0.000 %) |
| A2: git loop pushes ok/failed, clones ok/failed | 86/0, 86/0 | 82/0, 82/0 |
| fsck failures / acknowledged commits missing (A1+A2) | 0 / 0 | 0 / 0 |
| replacement pod Ready after a kill | 2-3 s | 2-3 s |
| B: kill mid-push (160 MB push), 4 runs | 3 interrupted, 1 force-delete still acked; ref always old or new (atomic); retry push ok after 13 s*; fsck clean; content matches | 2 interrupted (kill -9), 2 force-deletes still acked; ref atomic; retry ok after 13 s*; fsck clean; content matches |
| C: delete `minio-0`, then a push succeeds after | 4 s | 3 s |
| C: delete `postgres-0`, then a push succeeds after | 3 s | 4 s |
| C: acknowledged commits present after both | 5/5, fsck clean | 5/5, fsck clean |
| verdict | **PASS** | **PASS** |

\* The retry time is mostly the client re-packing 160 MB (about 10 s on this machine). The
per-repo advisory lock was released as soon as the killed pod's Postgres connection closed, and
no retry waited on a lock.

Phase B finds the pod running `git receive-pack`/`index-pack` through `/proc` and kills it
0-0.6 s later. Odd runs use `kill -9` on the `forge serve` process: a real crash, where the
container restarts and keeps its emptyDir cache with any partial state. Even runs use
`kubectl delete pod --grace-period=0 --force`.

### Runner pods killed mid-job (`tests/chaos/runner-kill.sh`)

Each case pushes a commit whose `on: push` workflow runs a 60 s job and kills the runner pod
running it 15 s into the job.

| | k3d | kind |
|---|---|---|
| graceful `kubectl delete pod`: run concluded | `success` 46 s after the kill (the runner drained the job, then exited) | `success` after 46 s |
| `--grace-period=0 --force`: run concluded | `failure` after 187 s, log "The runner has lost communication with the server" | `failure` after 183 s, same log line |
| `kill -9` of `forge runner` (container restarts, re-registers under the same name): run concluded | `failure` after 189 s, same log line | `failure` after 186 s |
| "re-run failed jobs" after force / crash | `success` after 63 s on another runner pod (both) | `success` after 63 s (both) |
| runs left queued / in_progress after each case and at the end | 0 | 0 |
| final push after all kills | `success` | `success` |
| verdict | **PASS** | **PASS** |

No run got stuck: a job whose runner vanished is failed by the scheduler's lost-job reaper
(3 min without a heartbeat + up to one 15 s tick) and can be re-run. The `crash` case shows the
same 3 min wait as `force`: a runner that re-registers under its old name replaces the runner
row, but its old in-progress job is still only reaped by the heartbeat timeout. Failing the
replaced runner's jobs at re-registration would cut that to seconds (suggested follow-up for
the runner owners; not changed here).

### forge pods killed during a chunked OCI blob upload (`tests/chaos/registry-kill.sh`)

`tests/chaos/oci_chunked_upload.py` pushes a 96 MB random blob as 12 PATCH chunks of 8 MB
(`Content-Range`), paced at 8 MB/s so a kill lands mid-chunk, through the NodePort. On any error
it does what a registry client should: GET the upload session and resume from its `Range`.
Then it PUTs `?digest=`, downloads the blob 3 times and checks sha256 and size, and the driver
checks `package_blobs.size` in Postgres.

| run | kill | k3d: client errors / resumes, result | kind: client errors / resumes, result |
|---|---|---|---|
| 1 | `kill -9` of `forge serve` in all 3 pods at once | 2 / 1 (resumed at 24 MB); 201, digest ok, 3/3 downloads ok | 1 / 1 (at 16 MB); 201, 3/3 ok |
| 2 | force-delete a random pod, twice | 0 / 0; 201, 3/3 ok | 0 / 0; 201, 3/3 ok |
| 3 | graceful delete a random pod, twice | 0 / 0; 201, 3/3 ok | 0 / 0; 201, 3/3 ok |
| 4 | `kill -9` in all 3 pods at once | 2 / 2 (at 24, 32 MB); 201, 3/3 ok | 1 / 1 (at 40 MB); 201, 3/3 ok |
| upload restarts needed / corrupt blobs / sessions left open | 0 / 0 / 0 | 0 / 0 / 0 |
| verdict | **PASS** | **PASS** |

A crashed pod loses only the chunk in flight: a chunk is committed to the session
(`package_uploads.parts`) only after it is fully in object storage, so the session's `Range`
is always a clean resume point, and the final digest is computed over the stored parts. Single
force/graceful deletes caused no client errors at all: the deleted pod still gets SIGTERM and
drains its in-flight PATCH (see below).

Findings:
- **No torn or corrupt repositories or blobs in any run.** Packs are uploaded before the
  `repo_storage` version bump, so an interrupted push leaves only orphan packs, which `forge gc`
  removes after an hour. A container that restarts after a crash discards stale local packs
  that are not in the manifest, and the following write succeeded every time.
- **`--force --grace-period=0` is not SIGKILL.** The kubelet still runs preStop and SIGTERM
  with a short grace period, so in-flight requests usually finish. That is why A2 had 0 errors
  and why some phase-B force-deletes still acknowledged their push. The `kill -9` runs are the
  real crash test.
- **Stock git can hang forever when the server dies mid-upload.** The client sits with its
  socket in CLOSE_WAIT and `git-remote-http` blocked on its pipe from send-pack (phase B, k3d run 2
  this time). `GIT_HTTP_LOW_SPEED_*` does not help. This is a client-side git
  behaviour, so the chaos scripts wrap every git call in a 120 s process-group watchdog (`tmo`).

Full reports: `tests/chaos/results/{k3d-20261018-003820,kind-20261018-010254}.md`,
`runner-{k3d-20261018-004548,kind-20261018-010824}.md`,
`registry-{k3d-20261018-005658,kind-20261018-011928}.md`. Previous pod-kill runs (2026-10-12,
same verdicts): `{k3d-20261012-175322,kind-20261012-180816}.md`.

## GitHub Actions runners (`deploy/base/forge-runner.yaml`)

Workflow jobs run in `forge runner` pods, never in the forge API pods (jobs run untrusted
code). The runner image is the `forge-runner` target of the same Dockerfile: the same binary
(`args: ["runner"]`, uid 10002) plus the CLI tools `run:` steps commonly expect on a Linux
runner (bash, git, curl, tar, gzip, xz, zstd, unzip/zip, jq, procps, findutils, diffutils, file,
openssh-client, python3-minimal). It runs as a separate Deployment that scales independently
(`kubectl -n forge scale deployment/forge-runner --replicas=N`). Jobs use the shell executor
inside the pod; there is no Docker daemon in the pod, so `container:` jobs and docker actions
are not available on Kubernetes.

```
forge pods ── scheduler (one pod at a time, Postgres advisory lock): cron triggers, lost-job reaping
   │  queued jobs in Postgres (claimed with FOR UPDATE SKIP LOCKED)
   ▼
forge-runner pods ──HTTP──> Service forge: /api/forge/runner/{register,acquire,jobs/{id}/...}
   │  checkout over git smart HTTP with the job's GITHUB_TOKEN
   └─ job work dir on an emptyDir (/work), HOME per job, shell executor (the pod is the sandbox)
```

- **Registration.** Pods register at start with the instance-wide registration token
  `FORGE_RUNNER_REGISTRATION_TOKEN` from the `forge-secrets` Secret. The server seeds the same
  token on start, so no one has to create one. The pod name is the runner name, so a restarted
  pod replaces its registration. The server prunes runners that have been offline for
  `FORGE_ACTIONS_RUNNER_OFFLINE_DAYS` (default 14). Repository, organization and enterprise runners can
  also be registered by hand with tokens from the REST API, or with
  `forge admin runner-token [--repo o/r | --org o] [--hours N]`.
- **No backend credentials.** The runner Deployment mounts only the registration token: no
  `DATABASE_URL` and no object storage keys. It does not mount a service account token
  (`automountServiceAccountToken: false`), it runs as non-root with a read-only root
  filesystem, and it writes only to `emptyDir` volumes (`/work`, `/tmp`).
- **Shutdown.** On SIGTERM a runner stops taking jobs and lets running jobs finish
  (`FORGE_RUNNER_DRAIN_SECS`, 600, within the pod's 660 s grace period). If a pod dies anyway,
  the scheduler marks its job failed after 3 minutes without heartbeats.
- **Labels.** Each runner has `self-hosted`, its OS (`Linux`), its architecture (`X64`/`ARM64`) and
  `FORGE_RUNNER_LABELS` (`kubernetes`). forge has no GitHub-hosted runners, so a
  `runs-on: ubuntu-latest` (or another `ubuntu-*`/`macos-*`/`windows-*` label) matches any runner.
  Every other label must be one of the runner's labels.
- **Executors.** `shell` (default) runs steps directly in the pod. `docker` runs every job in a
  container: the image comes from `container:`, or is mapped from `runs-on` (`ubuntu-24.04` ->
  `ubuntu:24.04`, otherwise `FORGE_RUNNER_DEFAULT_IMAGE`). The job directory is bind-mounted at the
  same path. The docker executor needs a Docker daemon, so on Kubernetes it needs a DinD sidecar
  (not shipped, because it requires privileged pods).

Outside Kubernetes:

```sh
TOKEN=$(gh api -X POST repos/OWNER/REPO/actions/runners/registration-token --jq .token)
forge runner --url http://forge.example:3000 --token "$TOKEN" --name my-runner \
  [--labels gpu,linux] [--executor docker] [--slots 2] [--config ~/.forge-runner.json]
forge runner --jitconfig "$ENCODED_JIT_CONFIG"     # one job, then exit (generate-jitconfig)
forge runner --url ... --config ~/.forge-runner.json --remove-token "$REMOVE_TOKEN"
```

## Packages: container and npm registries behind the Service

The OCI registry (`/v2/`) and the npm registry (`/npm/`) are served by the same `forge` pods
and the same Service as the API, so they need no extra Deployment or port. All registry state
lives in Postgres (packages, versions, tags, upload sessions) and object storage (content-addressed
blobs under `packages/blobs/sha256/`, upload chunks under `packages/uploads/`). Any pod serves
any request, including the chunks of one upload landing on different pods
(`uploads_hop_between_pods` in `crates/forge/tests/packages.rs`). `forge gc` (the CronJob) also
purges packages deleted more than 30 days ago and deletes unreferenced blobs and stale uploads.

Things a proxy or Ingress in front of the Service must allow:
- **Request bodies.** Layer uploads stream through forge into object storage; forge itself caps a
  blob at `FORGE_PACKAGES_MAX_BLOB_BYTES` (default 10 GiB) and an npm publish document at
  `FORGE_NPM_MAX_PUBLISH_BYTES` (default 512 MiB; it holds the base64 tarball). Raise or disable
  the proxy's limit, e.g. ingress-nginx `nginx.ingress.kubernetes.io/proxy-body-size: "0"`, and
  turn off request buffering (`proxy-request-buffering: "off"`) so large pushes stream instead of
  spooling to the proxy's disk. traefik has no default limit.
- **Timeouts.** A monolithic layer upload is one long request; use read/send timeouts of several
  minutes (`proxy-read-timeout`, `proxy-send-timeout`). Docker pushes in one PUT per layer by
  default.
- **Paths and headers.** Route `/v2/` and `/npm/` to forge unchanged (no prefix stripping) and
  pass `Authorization`, `Range`, `Content-Range` and the `WWW-Authenticate`, `Location`,
  `Docker-Content-Digest`, `Docker-Upload-UUID` and `OCI-Subject` response headers through.
- **`FORGE_BASE_URL` must be the externally visible URL.** The token realm in
  `WWW-Authenticate: Bearer realm="{base}/v2/token"` and the npm tarball URLs are built from it,
  and images are named `{host}/{owner}/{name}` after its host.
- **TLS.** docker only talks plain HTTP to `localhost`/`127.0.0.0/8` registries (or ones listed in
  `insecure-registries`). Anything else needs HTTPS at the LB/Ingress.

Clients:

```sh
docker login forge.example.com -u <login> --password-stdin <<< "$PAT"
docker push forge.example.com/<owner>/<image>:<tag>      # owner = user or organization
# .npmrc
@<owner>:registry=https://forge.example.com/npm/
//forge.example.com/npm/:_authToken=<PAT>
```

Local development note: Docker Desktop's daemon runs in a VM and cannot reach a forge listening on
the Mac's `localhost`. The packages area validated docker against a dev server by running a
socat container (`alpine/socat`, published on host port 3213 and forwarding to
`host.docker.internal:3211`) and setting `FORGE_BASE_URL=http://localhost:3213`, so the daemon's
`localhost:3213` reaches forge and the token realm points back through the same proxy. The k8s
NodePort (`localhost:8080`) is reachable from the daemon directly.

## Uninstall

`scripts/uninstall-deps.sh` deletes the `forge*` k3d/kind clusters and the baseline's `bench`
k3d cluster, the `forge:dev`/`forge-runner:dev` images (and any leftover
`localhost:8080|8088/...` smoke-test tags), forge volumes (`k3d-forge-*`, `k3d-bench-*`,
`forge-baseline-compat_*`), optionally every image pulled for the experiment (by ID, so untagged
leftovers go too): `rancher/k3s`, `ghcr.io/k3d-io/*`, `kindest/node`,
`cgr.dev/chainguard/minio{,-client}`, `postgres:16`, the build bases `rust:1-bookworm`,
`node:24-bookworm-slim`, `debian:{bookworm,trixie}-slim`, `docker/dockerfile`, the baseline's
Forgejo image, `alpine/socat`, the docker-executor images `ubuntu:24.04`/`alpine:3.20`, and the
earlier MinIO candidates `pgsty/minio`/`bitnamilegacy/minio*` (never `node:24-bookworm`, which
other projects on this machine use), and the build cache. It stops the `postgresql@16` brew
service, optionally deletes `/opt/homebrew/var/postgresql@16`, runs `rustup self uninstall`, and
removes the brew formulae listed in `scripts/.installed-by-forge`. It asks separately before
removing `gh` and `node`. It prints its plan and asks first; `--yes` skips every prompt. It never
touches Docker Desktop. (metrics-server on kind is pulled inside the kind node, not on the host,
so it goes with the cluster.)

# Deploying forge on Kubernetes (k3d, kind, any cluster)

forge runs on Postgres, S3-compatible object storage (MinIO) and Kubernetes, and nothing else.
k3d and kind use the **same image and the same rendered manifests**. Only the cluster-creation
block in `scripts/k8s/up.sh` differs. Everything runs locally: no registry, no cloud, no accounts.

## Layout

```
Dockerfile, .dockerignore     multi-stage: node (web/dist) -> rust (cargo build --release) -> debian-slim + git
deploy/base/                  namespace forge; postgres + minio StatefulSets, bucket Job, forge Deployment,
                              NodePort Service, PDB, HPA, gc CronJob, forge-runner Deployment (Actions),
                              ConfigMap + Secret generators
deploy/overlays/{k3d,kind}/   both are just `resources: [../../base]` (identical on purpose)
scripts/k8s/up.sh <flavor>    create cluster, build, import images, apply -k, wait, admin user + token, smoke test
scripts/k8s/down.sh <flavor>  delete the cluster
scripts/k8s/e2e.sh <flavor>   up + k6 suite + chaos + down
tests/load/                   k6: api-read.js, git-fetch.js, api-write.js; run.sh writes results/<cluster>-<date>.md
tests/chaos/pod-kill.sh       pod-kill chaos under load + git integrity checks; writes results/<flavor>-<date>.md
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
scripts/k8s/up.sh k3d                  # ~1 min with a warm build cache (a cold build adds ~3 min)
source data/k8s/k3d.env                # FORGE_URL, FORGE_TOKEN, KUBE_CONTEXT
tests/load/run.sh $FORGE_URL $FORGE_TOKEN          # DURATION=60s READ_VUS=20 GIT_VUS=10 WRITE_RATE=50
tests/chaos/pod-kill.sh k3d            # ~10 min; KILLS, INTERVAL, MIDPUSH_RUNS, PHASES, DEP_TARGETS
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

## Portability proof

| | k3d | kind |
|---|---|---|
| cluster | k3s v1.35.5, 1 server + 2 agents | Kubernetes v1.37.0, 1 control-plane + 2 workers |
| forge image id | `sha256:7f992a0ac668…` | `sha256:7f992a0ac668…` (same) |
| rendered manifests sha256 (`kubectl kustomize deploy/overlays/<flavor>`) | `e92370758aa8…` | `e92370758aa8…` (same) |
| up.sh: cluster + import + apply + rollout + smoke | 265 s (incl. 3 min Rust rebuild) / 56 s warm | 109 s |
| smoke: GET /user, POST /user/repos, git push, 3 fresh clones (HEAD match, fsck --strict) | pass | pass |

The pod-template hash differs (548c859bd9 vs 6d7b849b5) only because the two API server
versions apply different defaults to the same manifests.

## Load test results (k6 v2.3.0, 60 s per script, host: Apple silicon, Docker Desktop 10 CPU / 8 GB)

Mix: **api-read** = 20 closed-loop VUs over GET `/repos/{o}/{r}`, `/user`, `/user/repos`,
`/users/{u}`, `/users/{u}/repos`, `/languages`, `/rate_limit` ("core") plus issues, contents,
branches, commits and pulls ("optional": they may 404 until those areas merge, and they are
measured but not counted as errors). **git-fetch** = 10 VUs, each replaying a protocol-v2 clone:
info/refs, `ls-refs` and `fetch` (bodies match a `GIT_TRACE_PACKET` capture of git 2.50, see
`tests/load/fixtures/clone-v2.trace`) against 3 fixture repos with 120 commits each.
**api-write** = 50 ops/s fixed arrival rate: create repo, PATCH repo, create issue + comment
(the issue calls are optional).

| cluster | script | requests | req/s | errors | p50 | p95 | p99 | thresholds |
|---|---|---|---|---|---|---|---|---|
| k3d | api-read | 314,167 | 5,236 | 0 % | 2.1 ms | 11.5 ms | 26.0 ms | PASS |
| k3d | git-fetch | 35,562 (11,854 clones) | 401 | 0 % | 9.8 ms | 30.6 ms | 57.5 ms | PASS (clone p95 90 ms) |
| k3d | api-write | 2,998 | 50* | 0 % | 1.2 ms | 5.5 ms | 7.9 ms | PASS |
| kind | api-read | 329,011 | 5,483 | 0 % | 2.3 ms | 11.1 ms | 17.8 ms | PASS |
| kind | git-fetch | 25,413 (8,471 clones) | 422 | 0 % | 14.6 ms | 50.0 ms | 84.0 ms | PASS (clone p95 133 ms) |
| kind | api-write | 3,001 | 50 | 0 % | 1.3 ms | 6.7 ms | 12.0 ms | PASS |

\* In the k3d run the report shows 9.5 req/s because k6 only exited about 4 minutes after the
60 s scenario ended (`testRunDurationMs` 315 s, 2 dropped iterations, request max 101 ms). The
issued rate was 50/s. This looks like a host-side stall and did not reproduce on kind.

Thresholds: core read p95 < 250 ms and p99 < 600 ms; info/refs and ls-refs p95 < 300 ms;
fetch p95 < 1.5 s; clone p95 < 2 s; create-repo p95 < 800 ms; error rate < 1 %; checks > 99 %.
These runs share one laptop with k6 itself, so the numbers are a relative baseline, not a
capacity figure. During the earlier closed-loop write test (since replaced by a fixed rate),
Postgres hit its 2-CPU limit, so its limit was raised to 4 CPU.

Full reports: `tests/load/results/{k3d-20261012-174518,kind-20261012-180502}.md`.

## Chaos results (`tests/chaos/pod-kill.sh`)

Phases A1 and A2 each run for 110 s: k6 api-read (10 VUs) and git-fetch (4 VUs) with a 10 s
request timeout, plus a loop that commits ~40 KB, pushes, fresh-clones, runs
`git fsck --strict`, and checks that every acknowledged push is an ancestor of `origin/main`.
Six random forge pods are killed, one every 15 s.

| | k3d | kind |
|---|---|---|
| A1: graceful `kubectl delete pod` ×6: HTTP errors | **0 / 70,289** (0.000 %, limit 0.5 %) | **0 / 74,750** (0.000 %) |
| A1: git loop pushes ok/failed, clones ok/failed | 60/0, 60/0 | 66/0, 66/0 |
| A2: `--grace-period=0 --force` ×6: HTTP errors | **0 / 71,081** (0.000 %, limit 5 %) | **0 / 63,602** (0.000 %) |
| A2: git loop pushes ok/failed, clones ok/failed | 67/0, 67/0 | 63/0, 63/0 |
| fsck failures / acknowledged commits missing (A1+A2) | 0 / 0 | 0 / 0 |
| replacement pod Ready after a kill | 2-3 s | 2-3 s |
| B: kill mid-push (160 MB push), 4 runs | 3 interrupted, 1 completed before the kill; ref always old or new (atomic); retry push ok after 15-22 s*; fsck clean; content matches | 4 interrupted; ref old (atomic); retry ok after 13-21 s*; fsck clean; content matches |
| C: delete `minio-0`, then a push succeeds after | 3 s | 3 s |
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

Findings:
- **No torn or corrupt repositories in any run.** Packs are uploaded before the `repo_storage`
  version bump, so an interrupted push leaves only orphan packs, which `forge gc` removes after
  an hour. A container that restarts after a crash discards stale local packs that are not in
  the manifest, and the following write succeeded every time.
- **`--force --grace-period=0` is not SIGKILL.** The kubelet still runs preStop and SIGTERM
  with a short grace period, so in-flight requests usually finish. That is why A2 had 0 errors
  and why one phase-B force-delete still acknowledged its push. The `kill -9` runs are the real
  crash test.
- **Stock git can hang forever when the server dies mid-upload.** In one exploratory run the
  client sat with its socket in CLOSE_WAIT and `git-remote-http` blocked on its pipe from
  send-pack. `GIT_HTTP_LOW_SPEED_*` does not help. This is a client-side git behaviour, so the
  chaos scripts wrap every git call in a 120 s process-group watchdog (`tmo`).

Full reports: `tests/chaos/results/{k3d-20261012-175322,kind-20261012-180816}.md`.

## GitHub Actions runners (`deploy/base/forge-runner.yaml`)

Workflow jobs run in `forge runner` pods, never in the forge API pods (jobs run untrusted
code). The runner is the same image (`args: ["runner"]`), as a separate Deployment that
scales independently (`kubectl -n forge scale deployment/forge-runner --replicas=N`).

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

`scripts/uninstall-deps.sh` deletes `forge*` k3d/kind clusters, forge images and volumes,
optionally the cluster and build images and the build cache, stops the `postgresql@16` brew
service, optionally deletes `/opt/homebrew/var/postgresql@16`, runs `rustup self uninstall`, and
removes the brew formulae listed in `scripts/.installed-by-forge`. It asks separately before
removing `gh` and `node`. It prints its plan and asks first; `--yes` skips every prompt. It never
touches Docker Desktop.

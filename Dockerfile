# syntax=docker/dockerfile:1.7
# forge: runs on any Kubernetes (k3d, kind, managed clouds). Build natively for the host
# architecture (linux/arm64 on Apple silicon). Two targets from one build:
#   docker build -t forge:dev .                                 server (`forge serve`), the default
#   docker build --target forge-runner -t forge-runner:dev .    Actions runner (`forge runner`)
# Both contain the same binary; the runner image adds the tools that workflow `run:` steps
# expect on a shell-executor runner (curl, tar, gzip, xz, unzip, jq, ps, python3, ...).

############################ 1. web UI (static assets, embedded into the binary) ############
FROM node:24-bookworm-slim AS web
WORKDIR /web
COPY web/package.json web/package-lock.json ./
RUN --mount=type=cache,target=/root/.npm npm ci --no-audit --no-fund
COPY web/ ./
RUN npm run build

############################ 2. rust build ################################################
FROM rust:1-bookworm AS build
# aws-lc-rs builds AWS-LC from C sources (cmake + the base image's gcc and perl); clang and
# libclang cover bindgen should aws-lc-sys need to generate bindings for the target.
RUN apt-get update \
 && apt-get install -y --no-install-recommends cmake clang libclang-dev perl \
 && rm -rf /var/lib/apt/lists/*
WORKDIR /src
# Dependency layer: build all crates.io deps against a stub crate so that source edits do not
# recompile them. Only re-runs when Cargo.toml/Cargo.lock change.
COPY Cargo.toml Cargo.lock ./
COPY crates/forge/Cargo.toml crates/forge/Cargo.toml
RUN mkdir -p crates/forge/src web/dist \
 && echo 'fn main() {}' > crates/forge/src/main.rs \
 && echo '' > crates/forge/src/lib.rs \
 && echo 'fn main() {}' > crates/forge/build.rs \
 && cargo build --release -p forge --locked \
 && rm -rf crates/forge/src crates/forge/build.rs \
           target/release/forge target/release/deps/forge-* target/release/deps/libforge-* \
           target/release/.fingerprint/forge-* target/release/build/forge-*
# Real sources.
COPY crates/ crates/
COPY --from=web /web/dist web/dist
# The forge crate is large. With cargo's default parallelism its rustc (frontend plus 4 LLVM
# codegen units in parallel) grows past the ~7.8 GB of a default Docker Desktop VM and was
# OOM-killed (also with LTO off). One jobserver token (-j1; the dependencies above are already
# built in parallel) makes rustc optimise one codegen unit at a time: same profile, same binary,
# peak ~6.8 GB, ~9 min instead of ~4. Raise CARGO_BUILD_JOBS on a VM with >= 12 GB.
ARG CARGO_PROFILE_RELEASE_LTO=thin CARGO_PROFILE_RELEASE_CODEGEN_UNITS=4 CARGO_BUILD_JOBS=1
ENV MALLOC_ARENA_MAX=2 CARGO_BUILD_JOBS=$CARGO_BUILD_JOBS CARGO_PROFILE_RELEASE_LTO=$CARGO_PROFILE_RELEASE_LTO CARGO_PROFILE_RELEASE_CODEGEN_UNITS=$CARGO_PROFILE_RELEASE_CODEGEN_UNITS
RUN cargo build --release -p forge --locked \
 && install -m 0755 target/release/forge /usr/local/bin/forge

############################ 3. runtime base (shared by both targets) ######################
# trixie, not bookworm: forge needs git >= 2.40 (`git merge-tree --merge-base`, used by rebase
# merges and the `rebaseable` check); bookworm ships 2.39.5, trixie 2.47. (The binary is built
# on bookworm's older glibc, so it runs unchanged on trixie.) bash comes with the base image;
# the k8s init containers use its /dev/tcp.
FROM debian:trixie-slim AS runtime
RUN apt-get update \
 && apt-get install -y --no-install-recommends git ca-certificates tini \
 && rm -rf /var/lib/apt/lists/* \
 && groupadd --system --gid 10001 forge \
 && useradd --system --uid 10001 --gid forge --home-dir /var/lib/forge --create-home forge \
 && mkdir -p /var/cache/forge && chown forge:forge /var/cache/forge
COPY --from=build /usr/local/bin/forge /usr/local/bin/forge
WORKDIR /var/lib/forge
ENV FORGE_LISTEN=0.0.0.0:3000 \
    FORGE_CACHE_DIR=/var/cache/forge \
    RUST_LOG=forge=info,tower_http=warn
# tini reaps the git child processes forge spawns and forwards SIGTERM for graceful shutdown.
ENTRYPOINT ["/usr/bin/tini", "--", "forge"]

############################ 4a. forge-runner: Actions runner (shell executor) ##############
# Workflow `run:` steps execute directly in this container (the pod is the sandbox), so it
# carries the usual CLI tools of a Linux runner. actions/checkout, upload/download-artifact
# and cache are built into `forge runner` (git, tar, gzip). No docker CLI: the docker executor
# needs a daemon, which a pod only gets with a privileged DinD sidecar (not shipped).
FROM runtime AS forge-runner
RUN apt-get update \
 && apt-get install -y --no-install-recommends curl tar gzip xz-utils zstd unzip zip jq \
      procps findutils diffutils file openssh-client python3-minimal \
 && rm -rf /var/lib/apt/lists/* \
 && groupadd --system --gid 10002 runner \
 && useradd --system --uid 10002 --gid runner --home-dir /home/runner --create-home --shell /bin/bash runner \
 && mkdir -p /work && chown runner:runner /work
USER 10002:10002
WORKDIR /home/runner
ENV FORGE_RUNNER_WORKDIR=/work \
    RUST_LOG=forge=info
CMD ["runner"]

############################ 4b. forge: server (default target, so it is the last stage) ####
FROM runtime AS forge
USER 10001:10001
EXPOSE 3000
CMD ["serve"]

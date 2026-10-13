# syntax=docker/dockerfile:1.7
# forge: one image, runs on any Kubernetes (k3d, kind, managed clouds). Build natively for the
# host architecture (linux/arm64 on Apple silicon): `docker build -t forge:dev .`

############################ 1. web UI (static assets, embedded into the binary) ############
FROM node:24-bookworm-slim AS web
WORKDIR /web
COPY web/package.json web/package-lock.json ./
RUN --mount=type=cache,target=/root/.npm npm ci --no-audit --no-fund
COPY web/ ./
RUN npm run build

############################ 2. rust build ################################################
FROM rust:1-bookworm AS build
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
RUN cargo build --release -p forge --locked \
 && install -m 0755 target/release/forge /usr/local/bin/forge

############################ 3. runtime ###################################################
# trixie, not bookworm: forge needs git >= 2.40 (`git merge-tree --merge-base`, used by rebase
# merges and the `rebaseable` check). bookworm ships git 2.39.5, where rebase merges failed with
# "This branch can't be rebased". trixie ships 2.47. (The binary is built on bookworm's older
# glibc, so it runs unchanged on trixie.)
FROM debian:trixie-slim AS runtime
RUN apt-get update \
 && apt-get install -y --no-install-recommends git ca-certificates tini \
 && rm -rf /var/lib/apt/lists/* \
 && groupadd --system --gid 10001 forge \
 && useradd --system --uid 10001 --gid forge --home-dir /var/lib/forge --create-home forge \
 && mkdir -p /var/cache/forge && chown forge:forge /var/cache/forge
COPY --from=build /usr/local/bin/forge /usr/local/bin/forge
USER 10001:10001
WORKDIR /var/lib/forge
ENV FORGE_LISTEN=0.0.0.0:3000 \
    FORGE_CACHE_DIR=/var/cache/forge \
    RUST_LOG=forge=info,tower_http=warn
EXPOSE 3000
# tini reaps the git child processes forge spawns and forwards SIGTERM for graceful shutdown.
ENTRYPOINT ["/usr/bin/tini", "--", "forge"]
CMD ["serve"]

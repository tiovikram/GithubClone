#!/usr/bin/env bash
# (Re)starts a local dev forge on :3000 using native Postgres and file storage under data/.
set -euo pipefail
cd "$(dirname "$0")/.."
source scripts/env.sh
# Only this checkout's server: other worktrees run their own forge on other ports.
pkill -f "$PWD/target/debug/forge serve" 2>/dev/null || true
cargo build -q -p forge
mkdir -p data
RUST_LOG=${RUST_LOG:-forge=info} nohup "$PWD/target/debug/forge" serve > data/server.log 2>&1 &
for _ in $(seq 1 50); do curl -sf localhost:3000/healthz >/dev/null && echo "forge up on :3000" && exit 0; sleep 0.2; done
echo "forge failed to start; see data/server.log" >&2; exit 1

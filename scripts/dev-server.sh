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
PID=$!
for _ in $(seq 1 50); do
  # The health check must come from the process we just started, not a stale server.
  kill -0 "$PID" 2>/dev/null || { echo "forge exited; see data/server.log" >&2; tail -3 data/server.log >&2; exit 1; }
  curl -sf localhost:3000/healthz >/dev/null && echo "forge up on :3000 (pid $PID)" && exit 0; sleep 0.2
done
echo "forge failed to start; see data/server.log" >&2; exit 1

#!/usr/bin/env bash
# Starts a private forge backend for one web-UI area plus a Vite dev server in front of it.
#
#   scripts/web-area-server.sh <area> <backend-port> <vite-port>     # (re)start both
#   scripts/web-area-server.sh <area> <backend-port> <vite-port> stop
#
# Backend: DB forge_web_<area> (created if missing), storage under data/web-<area>/, log
# data/web-<area>/server.log. Binary: $FORGE_BIN, else this checkout's target/debug/forge if it
# exists, else a copy of the main checkout's build (data/web-<area>/forge).
# Vite: http://localhost:<vite-port>, proxying the API and git to the backend; log
# data/web-<area>/vite.log. Use the Vite URL for the browser and the e2e (--base).
set -euo pipefail
cd "$(dirname "$0")/.."
ROOT=$PWD
AREA=${1:?area}; PORT=${2:?backend port}; VPORT=${3:?vite port}; CMD=${4:-start}
source scripts/env.sh
export PGPASSWORD=forge
D=$ROOT/data/web-$AREA
mkdir -p "$D"
pkill -f "forge serve.*#web-$AREA" 2>/dev/null || true
[ -f "$D/vite.pid" ] && { kill "$(cat "$D/vite.pid")" 2>/dev/null || true; rm -f "$D/vite.pid"; }
lsof -ti tcp:"$PORT" -sTCP:LISTEN 2>/dev/null | xargs kill 2>/dev/null || true
lsof -ti tcp:"$VPORT" -sTCP:LISTEN 2>/dev/null | xargs kill 2>/dev/null || true
[ "$CMD" = stop ] && { echo "stopped $AREA"; exit 0; }

MAIN=$(git worktree list --porcelain | awk '/^worktree /{print $2; exit}')
BIN=${FORGE_BIN:-}
if [ -z "$BIN" ]; then
  if [ -x "$ROOT/target/debug/forge" ]; then BIN=$ROOT/target/debug/forge
  else
    [ -x "$D/forge" ] || cp "$MAIN/target/debug/forge" "$D/forge"
    BIN=$D/forge
  fi
fi
DB=forge_web_$AREA
psql -h localhost -U forge -d postgres -tAc "SELECT 1 FROM pg_database WHERE datname='$DB'" | grep -q 1 ||
  createdb -h localhost -U forge -O forge "$DB"

export FORGE_LISTEN=127.0.0.1:$PORT FORGE_BASE_URL=http://localhost:$VPORT \
  DATABASE_URL=postgres://forge:forge@localhost/$DB \
  FORGE_STORAGE="file://$D/objects" FORGE_CACHE_DIR="$D/cache" \
  RUST_LOG=${RUST_LOG:-forge=info}
# The trailing comment-like arg makes this area's process easy to find with pkill -f.
nohup bash -c 'exec -a "forge serve #web-'"$AREA"'" "$0" serve' "$BIN" > "$D/server.log" 2>&1 &
PID=$!
for _ in $(seq 1 100); do
  kill -0 "$PID" 2>/dev/null || { echo "forge exited; see $D/server.log" >&2; tail -5 "$D/server.log" >&2; exit 1; }
  curl -sf "localhost:$PORT/healthz" >/dev/null && break
  sleep 0.2
done
curl -sf "localhost:$PORT/healthz" >/dev/null || { echo "forge did not come up" >&2; exit 1; }

cd web
[ -d node_modules ] || npm install --silent
FORGE_BACKEND=http://localhost:$PORT nohup npx vite --port "$VPORT" --strictPort > "$D/vite.log" 2>&1 &
echo $! > "$D/vite.pid"
for _ in $(seq 1 100); do
  curl -sf "localhost:$VPORT/" >/dev/null && { echo "$AREA: forge :$PORT (pid $PID, $BIN), vite http://localhost:$VPORT"; exit 0; }
  sleep 0.2
done
echo "vite did not come up; see $D/vite.log" >&2; exit 1

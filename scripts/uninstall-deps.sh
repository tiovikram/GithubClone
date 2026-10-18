#!/usr/bin/env bash
# Removes everything installed or created for the forge experiment. Paired with
# scripts/install-deps.sh. Lists what it will do and asks before each destructive group.
#   scripts/uninstall-deps.sh          interactive
#   scripts/uninstall-deps.sh --yes    no prompts (also deletes the Postgres data dir)
# Never touches Docker Desktop itself (it was installed before the experiment).
set -uo pipefail
YES=0
for a in "$@"; do case "$a" in -y|--yes) YES=1;; -h|--help) sed -n '2,7p' "$0"; exit 0;; *) echo "unknown arg $a" >&2; exit 2;; esac; done
HERE=$(cd "$(dirname "$0")" && pwd)
# The list lives in the main checkout (gitignored); fall back to this checkout's copy.
LIST=""
for c in "$HERE/.installed-by-forge" "$(git -C "$HERE" rev-parse --path-format=absolute --git-common-dir 2>/dev/null | sed 's#/\.git$##')/scripts/.installed-by-forge"; do
  [ -f "$c" ] && { LIST=$c; break; }
done
PG_DATA=/opt/homebrew/var/postgresql@16

confirm() { # $1 prompt
  [ $YES = 1 ] && return 0
  read -r -p "$1 [y/N] " ans; [[ "$ans" =~ ^[Yy] ]]
}
have() { command -v "$1" >/dev/null 2>&1; }

FORMULAE=()
[ -n "$LIST" ] && while read -r f; do [ -n "$f" ] && FORMULAE+=("$f"); done < <(sort -u "$LIST")
CLUSTERS_K3D=(); CLUSTERS_KIND=()
have k3d && while read -r c; do [ -n "$c" ] && CLUSTERS_K3D+=("$c"); done < <(k3d cluster list -o json 2>/dev/null | jq -r '.[].name' 2>/dev/null | grep '^forge')
have kind && while read -r c; do [ -n "$c" ] && CLUSTERS_KIND+=("$c"); done < <(kind get clusters 2>/dev/null | grep '^forge')
IMAGES=(); VOLUMES=()
if have docker; then
  while read -r i; do [ -n "$i" ] && IMAGES+=("$i"); done < <(docker images --format '{{.Repository}}:{{.Tag}}' | grep -E '^forge(:|-)' )
  # Volumes created by k3d (k3d-forge-images) and kind/k3d node containers of forge clusters.
  while read -r v; do [ -n "$v" ] && VOLUMES+=("$v"); done < <(docker volume ls -q | grep -E '^(k3d-forge|forge)')
fi

echo "This will remove:"
echo "  k3d clusters:     ${CLUSTERS_K3D[*]:-(none)}"
echo "  kind clusters:    ${CLUSTERS_KIND[*]:-(none)}"
echo "  docker images:    ${IMAGES[*]:-(none)}  (plus k3d/kind node and dependency images, asked separately)"
echo "  docker volumes:   ${VOLUMES[*]:-(none)}"
echo "  brew formulae:    ${FORMULAE[*]:-(none; ${LIST:-no .installed-by-forge found})}"
echo "                    NOTE: gh and node may be useful outside this experiment."
echo "  brew service:     postgresql@16 (stop); data dir $PG_DATA (asked separately)"
echo "  rust toolchain:   rustup self uninstall (~/.rustup, ~/.cargo)"
echo "  Docker Desktop is NOT touched."
confirm "Proceed?" || { echo "aborted"; exit 1; }

# 1. clusters
for c in "${CLUSTERS_K3D[@]:-}"; do [ -n "$c" ] && k3d cluster delete "$c"; done
for c in "${CLUSTERS_KIND[@]:-}"; do [ -n "$c" ] && kind delete cluster --name "$c"; done

# 2. docker images + volumes
if have docker; then
  for i in "${IMAGES[@]:-}"; do [ -n "$i" ] && docker rmi -f "$i"; done
  for v in "${VOLUMES[@]:-}"; do [ -n "$v" ] && docker volume rm -f "$v"; done
  EXTRA=$(docker images --format '{{.Repository}}:{{.Tag}}' | grep -E '^(rancher/k3s|ghcr.io/k3d-io/|kindest/node|cgr.dev/chainguard/minio|postgres:16|rust:1-bookworm|node:24-bookworm-slim|debian:bookworm-slim|codeberg.org/forgejo/forgejo|grafana/k6|ubuntu:24.04|alpine:3.20)' | tr '\n' ' ')
  # (ubuntu:24.04 and alpine:3.20 are pulled by the Actions docker executor of `forge runner`.)
  if [ -n "$EXTRA" ] && confirm "Also remove images pulled for the clusters/build: $EXTRA?"; then
    # shellcheck disable=SC2086
    docker rmi -f $EXTRA
  fi
  if confirm "Prune the Docker build cache (forge builds)?"; then docker builder prune -af; fi
fi

# 3. postgres service + data
if have brew && brew list --formula postgresql@16 >/dev/null 2>&1; then
  brew services stop postgresql@16 || true
  if [ -d "$PG_DATA" ] && confirm "Delete Postgres data dir $PG_DATA (all local databases)?"; then
    rm -rf "$PG_DATA"
  fi
fi

# 3b. build outputs inside this repo (cargo target, node_modules, web/dist, local data)
ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
LOCAL_DIRS=()
for d in target web/node_modules tools/conformance/node_modules web/dist data; do
  [ -e "$ROOT_DIR/$d" ] && LOCAL_DIRS+=("$ROOT_DIR/$d")
done
if [ ${#LOCAL_DIRS[@]} -gt 0 ] && confirm "Delete repo-local build outputs (${LOCAL_DIRS[*]})?"; then
  rm -rf "${LOCAL_DIRS[@]}"
fi

# 4. rust toolchain
if [ -x "$HOME/.cargo/bin/rustup" ] || have rustup; then
  RUSTUP=$( [ -x "$HOME/.cargo/bin/rustup" ] && echo "$HOME/.cargo/bin/rustup" || command -v rustup )
  "$RUSTUP" self uninstall -y || true
fi

# 5. brew formulae
if [ ${#FORMULAE[@]} -gt 0 ] && have brew; then
  KEEP=()
  if [ $YES = 0 ]; then
    for f in gh node; do
      if printf '%s\n' "${FORMULAE[@]}" | grep -qx "$f" && ! confirm "Uninstall $f too?"; then KEEP+=("$f"); fi
    done
  fi
  REMOVE=()
  for f in "${FORMULAE[@]}"; do printf '%s\n' "${KEEP[@]:-}" | grep -qx "$f" || REMOVE+=("$f"); done
  [ ${#REMOVE[@]} -gt 0 ] && brew uninstall --ignore-dependencies "${REMOVE[@]}"
  brew autoremove || true
  # Keep listing anything the user chose to keep, so a later run can still remove it.
  if [ -n "$LIST" ]; then
    if [ ${#KEEP[@]} -gt 0 ]; then printf '%s\n' "${KEEP[@]}" > "$LIST"; else rm -f "$LIST"; fi
  fi
fi
echo "done."

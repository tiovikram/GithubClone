#!/usr/bin/env bash
# Installs every tool used by the forge experiment. Paired with scripts/uninstall-deps.sh.
set -euo pipefail
FORMULAE=(rustup gh k3d kind helm k6 node postgresql@16 jq)
for f in "${FORMULAE[@]}"; do
  if brew list --formula "$f" >/dev/null 2>&1; then echo "already installed: $f"; else
    brew install "$f" && echo "$f" >> "$(dirname "$0")/.installed-by-forge"; fi
done
# Rust toolchain via rustup (lives in ~/.rustup and ~/.cargo)
"$(brew --prefix rustup)/bin/rustup-init" -y --no-modify-path --default-toolchain stable >/dev/null 2>&1 || true
echo "Add to PATH: export PATH=\"$HOME/.cargo/bin:$(brew --prefix rustup)/bin:\$PATH\""

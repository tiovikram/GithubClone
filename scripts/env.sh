# source scripts/env.sh — puts forge's toolchain on PATH
export PATH="/opt/homebrew/opt/rustup/bin:/opt/homebrew/opt/postgresql@16/bin:$HOME/.cargo/bin:$PATH"
export DATABASE_URL="${DATABASE_URL:-postgres://forge:forge@localhost/forge}"
export DATABASE_URL_TEST="${DATABASE_URL_TEST:-postgres://forge:forge@localhost/forge_test}"

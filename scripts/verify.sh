#!/bin/sh
# What "done" means for a change: formatted, lint-clean and tested.
#   scripts/verify.sh              everything, as CI runs it
#   scripts/verify.sh quick        format and lint only
#   scripts/verify.sh quick <crate>  format, lint and the tests of one crate
set -eu

cd "$(dirname "$0")/.."

# The window's interface is built by Vite; the Rust crate only needs the folder to exist.
mkdir -p app/dist
[ -f app/dist/index.html ] || echo '<!doctype html><title>Leme Pier</title>' > app/dist/index.html

mode="${1:-full}"
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
# Windows code is only compiled on Windows, so lint the core for it here when its target is installed
# (`rustup target add x86_64-pc-windows-gnu`). The window's crate and the TLS stack need a Windows toolchain: only CI builds them.
if rustup target list --installed 2>/dev/null | grep -q '^x86_64-pc-windows-gnu$'; then
  cargo clippy -p leme-core --no-default-features --all-targets --target x86_64-pc-windows-gnu -- -D warnings
fi

case "$mode" in
  quick) [ $# -ge 2 ] && cargo test -p "$2" ;;
  full) cargo test --workspace ;;
  *) echo "usage: scripts/verify.sh [quick [crate]]" >&2; exit 2 ;;
esac

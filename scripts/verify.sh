#!/bin/sh
# What "done" means for a change: formatted, lint-clean and tested.
#   scripts/verify.sh              everything, as CI runs it
#   scripts/verify.sh quick        format and lint only
#   scripts/verify.sh quick <crate>  format, lint and the tests of one crate
set -eu

cd "$(dirname "$0")/.."

mode="${1:-full}"
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings

case "$mode" in
  quick) [ $# -ge 2 ] && cargo test -p "$2" ;;
  full) cargo test --workspace ;;
  *) echo "usage: scripts/verify.sh [quick [crate]]" >&2; exit 2 ;;
esac

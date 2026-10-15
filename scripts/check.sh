#!/usr/bin/env bash
# Runs every check that must pass before committing.
set -euo pipefail
cd "$(dirname "$0")/.."

export RUSTFLAGS="-D warnings"

cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
# Release compiles out debug-only code; catch anything left unused.
cargo clippy --workspace --all-targets --release -- -D warnings
# Debug: the realtime allocation guard is active.
cargo test --workspace
# Release: the guard compiles away; behaviour must not change.
cargo test --workspace --release

echo "All checks passed."

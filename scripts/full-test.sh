#!/usr/bin/env bash
# The full test run: everything in check.sh, plus the headless UI tests
# that click through the app (crates/daw-ui/src/ui_tests, one per entry in
# docs/behaviours.md). Slower to build, so run it occasionally: when a
# feature is finished, after changing shared input or editing code, before
# a milestone, or when asked. Not on every commit.
set -euo pipefail
cd "$(dirname "$0")/.."

./scripts/check.sh

export RUSTFLAGS="-D warnings"
cargo clippy -p daw-ui --all-targets --features ui-tests -- -D warnings
cargo test -p daw-ui --features ui-tests ui_tests
# Frame-time budgets on a full-size song: release, one test at a time.
cargo test --release -p daw-ui --features ui-tests perf_ -- --test-threads=1 --nocapture

echo "Full test run passed."

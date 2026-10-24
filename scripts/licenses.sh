#!/usr/bin/env bash
# Writes licenses/NOTICES.md: every third-party library in the app with its
# licence and copyright notice (settings in about.toml, layout in about.hbs).
# Run after adding, removing or updating dependencies, and before a release.
# Needs cargo-about: cargo install cargo-about --locked --features cli
set -euo pipefail
cd "$(dirname "$0")/.."

cargo about generate about.hbs -o licenses/NOTICES.md
echo "Wrote licenses/NOTICES.md."

#!/bin/bash
set -e

echo "Running formatting check..."
cargo fmt --all -- --check

# --workspace is explicit for the same reason it is in .github/workflows/rust.yml:
# the manifest sets default-members to exclude the Tauri desktop crate, and a
# bare invocation respects that. This script is the pre-PR gate, so leaving it
# bare would stop it covering the crate CI still builds.
echo "Running Clippy..."
cargo clippy --workspace --all-targets --all-features

echo "Running tests..."
cargo test --workspace --all-features

echo "All checks passed!"

#!/usr/bin/env bash
set -euo pipefail

repo_dir=$(cd "$(dirname "$0")/.." && pwd)
temp_dir=$(mktemp -d /tmp/ducktape-view-fixtures.XXXXXX)
trap 'rm -rf "$temp_dir"' EXIT
mkdir -p "$temp_dir/src"
cp "$repo_dir/fixtures/producer-generator/Cargo.toml" "$temp_dir/Cargo.toml"
cp "$repo_dir/fixtures/producer-generator/src/main.rs" "$temp_dir/src/main.rs"

mode=${1:---check}
fixture_target_dir=${CARGO_TARGET_DIR:-$repo_dir/target/producer-fixtures}
RUSTC_WRAPPER= CARGO_BUILD_JOBS=2 CARGO_TARGET_DIR="$fixture_target_dir" \
    cargo generate-lockfile --manifest-path "$temp_dir/Cargo.toml"
RUSTC_WRAPPER= CARGO_BUILD_JOBS=2 CARGO_TARGET_DIR="$fixture_target_dir" \
    cargo run --locked --manifest-path "$temp_dir/Cargo.toml" -- "$mode" "$repo_dir"

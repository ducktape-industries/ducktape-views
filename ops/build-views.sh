#!/usr/bin/env bash
# Run from the checkout root. Match guest-builder's canonical source prefixes;
# encoded flags preserve paths containing spaces and replace ambient Rust flags.
set -euo pipefail
repo=$(pwd -P)
# The accessibility gate: no component is built while a view draws a tree
# assistive technology cannot read. Every view's tests assert that
# `view_wire::accessibility_faults` is empty over EVERY tree they render, in
# the helper each frame passes through, so the gate is the view tests
# themselves, unfiltered: a name filter would skip a view that forgot the
# name. A native test run, before the rust flags below and never for the wasm
# target, so it does not move a component's bytes.
"${CARGO:-cargo}" test --locked --manifest-path "$repo/Cargo.toml" --workspace
cargo_home=$(cd "${CARGO_HOME:-$HOME/.cargo}" && pwd -P)
# A toolchain installed without rustup (the agent guest's /opt/rust) has no
# rustup home; its sysroot is the prefix rust's own paths live under.
rustup_home=$(cd "${RUSTUP_HOME:-$HOME/.rustup}" 2>/dev/null && pwd -P || rustc --print sysroot)
view_target=${CARGO_TARGET_DIR:-$repo/target}
mkdir -p "$view_target"
view_target=$(cd "$view_target" && pwd -P)
# Cargo hashes a path dependency's ABSOLUTE location into that package's
# `-C metadata` whenever the package sits outside the workspace root, and
# `-C metadata` seeds every symbol hash the crate emits. A view's bytes would
# otherwise depend on where the checkout lives — and two hosts could not
# build one founding set. Compile through one constant path so that location
# is the same string everywhere. The path is a single global name, so one
# build owns it at a time; `ops/views-repro-check.sh` names it too, to prove
# it never reaches a component's bytes.
source_root=/var/tmp/ducktape-view-root
lock="$source_root.lock"
# The holder's pid reclaims the lock after a build dies without its trap.
until mkdir "$lock" 2>/dev/null; do
  holder=$(cat "$lock/pid" 2>/dev/null || true)
  [ -n "$holder" ] && kill -0 "$holder" 2>/dev/null || rm -rf "$lock"
  sleep 1
done
echo $$ > "$lock/pid"
trap 'rm -rf "$lock"' EXIT
[ ! -e "$source_root" ] || [ -L "$source_root" ] || {
  echo "$source_root exists and is not a symlink" >&2
  exit 1
}
ln -sfn "$repo" "$source_root"
printf -v CARGO_ENCODED_RUSTFLAGS '%s\x1f%s\x1f%s\x1f%s' \
  "--remap-path-prefix=$cargo_home=/cargo" \
  "--remap-path-prefix=$rustup_home=/rustup" \
  "--remap-path-prefix=$source_root=/ducktape" \
  "--remap-path-prefix=$view_target=/view-builder"
export CARGO_ENCODED_RUSTFLAGS
export CARGO_TARGET_DIR="$view_target"
unset RUSTFLAGS
# wit-bindgen emits the component type into each Rust cdylib. No UI compiler,
# browser import adapters, or embedded guest bytes are involved.
packages=()
while (($#)); do
  case "$1" in
    -p) packages+=("$2"); shift 2 ;;
    # every workspace member whose package name ends in `-view`, sorted
    --all)
      names=$(sed -n 's/^name = "\(.*-view\)"$/\1/p' "$repo"/*/Cargo.toml | sort)
      for name in $names; do packages+=("$name"); done
      shift ;;
    *) echo "expected -p <view-package> or --all, got $1" >&2; exit 1 ;;
  esac
done
(("${#packages[@]}")) || { echo "no view packages selected" >&2; exit 1; }
arguments=()
for package in "${packages[@]}"; do arguments+=(-p "$package"); done
"${CARGO:-cargo}" build --locked --manifest-path "$source_root/Cargo.toml" --release \
  --target wasm32-unknown-unknown "${arguments[@]}"
mkdir -p "$repo/target/views"
for package in "${packages[@]}"; do
  library=${package//-/_}
  wasm-tools component new "$view_target/wasm32-unknown-unknown/release/$library.wasm" \
    -o "$repo/target/views/$library.wasm"
done
wasm-tools --version > "$repo/target/views/WASM_TOOLS_VERSION"
# Each view again as the ONE unit a node's registry takes: `<id>/view.wasm`
# beside that view's `assets/`, when it has any — the shape core's founding set
# and `modules.update` read. Rebuilt whole, so a removed asset does not linger.
for package in "${packages[@]}"; do
  id=${package%-view}
  unit="$repo/target/views/registry/$id"
  rm -rf "$unit"
  mkdir -p "$unit"
  cp "$repo/target/views/${package//-/_}.wasm" "$unit/view.wasm"
  [ ! -d "$repo/$id/assets" ] || cp -R "$repo/$id/assets" "$unit/assets"
done

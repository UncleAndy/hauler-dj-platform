#!/usr/bin/env bash
# Rust gates: toolchain pin, formatting, clippy (-D warnings), workspace tests.
# Runs inside the flake dev shell; scripts/check.sh enters the shell for you.
set -euo pipefail
cd "$(dirname "$0")/.."

if [ -z "${DJ_PLATFORM_DEVSHELL:-}" ]; then
  echo "warning: not inside the dev shell; run scripts/check.sh instead" >&2
fi

pin="$(sed -n 's/^channel[[:space:]]*=[[:space:]]*"\(.*\)"/\1/p' rust-toolchain.toml)"
actual="$(rustc --version | cut -d' ' -f2)"
if [ -z "$pin" ] || [ "$pin" != "$actual" ]; then
  echo "toolchain pin mismatch: rust-toolchain.toml says '$pin', rustc --version says '$actual'" >&2
  exit 1
fi
echo "toolchain: rustc $actual (matches rust-toolchain.toml)"

cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace

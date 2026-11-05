#!/usr/bin/env bash
# Build the Qt/QML shell and run it.
#   scripts/ui.sh --smoke       build, load the scene under the offscreen platform, exit
#   scripts/ui.sh               build and start the window (needs a display)
#   BUILD_DIR=build-dbg scripts/ui.sh --build-only
set -euo pipefail
cd "$(dirname "$0")/.."

if [ -z "${DJ_PLATFORM_DEVSHELL:-}" ]; then
  # Plain `nix develop` uses the git tree, so the ignored target/ is not copied into the
  # nix store (a path: reference copies it and then fails on files owned by another uid).
  exec nix develop --command bash "$0" "$@"
fi

build_dir="${BUILD_DIR:-build}"

cmake -S . -B "$build_dir" -G Ninja
cmake --build "$build_dir"

case "${1:-}" in
  --build-only) exit 0 ;;
  --smoke)
    QT_QPA_PLATFORM=offscreen "$build_dir/apps/dj-ui/dj-ui" --smoke
    ;;
  --engine-check)
    # The A06 acceptance, without a display or a sound card: run the real engine on a null sink,
    # let the real UI press Play, and require the ack and the published state to agree.
    engine_bin="${CARGO_TARGET_DIR:-$HOME/.cache/dj-platform-target}/debug/dj-engine"
    if [ ! -x "$engine_bin" ]; then
      echo "ui.sh: dj-engine is not built at $engine_bin" >&2
      exit 1
    fi
    run_dir="$(mktemp -d)"
    socket="$run_dir/engine.sock"
    "$engine_bin" --null-sink --socket "$socket" --block 256 &
    engine_pid=$!
    cleanup() {
      kill "$engine_pid" 2>/dev/null || true
      wait "$engine_pid" 2>/dev/null || true
      rm -rf "$run_dir"
    }
    trap cleanup EXIT
    for _ in $(seq 1 100); do
      [ -S "$socket" ] && break
      sleep 0.05
    done
    if [ ! -S "$socket" ]; then
      echo "ui.sh: the engine did not create $socket" >&2
      exit 1
    fi
    QT_QPA_PLATFORM=offscreen "$build_dir/apps/dj-ui/dj-ui" --connect "$socket" --self-check
    ;;
  *)
    exec "$build_dir/apps/dj-ui/dj-ui" "$@"
    ;;
esac

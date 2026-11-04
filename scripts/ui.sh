#!/usr/bin/env bash
# Build the Qt/QML shell and run it.
#   scripts/ui.sh --smoke       build, load the scene under the offscreen platform, exit
#   scripts/ui.sh               build and start the window (needs a display)
#   BUILD_DIR=build-dbg scripts/ui.sh --build-only
set -euo pipefail
cd "$(dirname "$0")/.."

if [ -z "${DJ_PLATFORM_DEVSHELL:-}" ]; then
  exec nix develop "path:$PWD" --command bash "$0" "$@"
fi

build_dir="${BUILD_DIR:-build}"

cmake -S . -B "$build_dir" -G Ninja
cmake --build "$build_dir"

case "${1:-}" in
  --build-only) exit 0 ;;
  --smoke)
    QT_QPA_PLATFORM=offscreen "$build_dir/apps/dj-ui/dj-ui" --smoke
    ;;
  *)
    exec "$build_dir/apps/dj-ui/dj-ui" "$@"
    ;;
esac

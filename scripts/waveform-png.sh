#!/usr/bin/env bash
# Render a track's waveform to a PNG with the deck's own colours.
#
# The point is to look at the colouring without a sound card, a controller or even a display:
# the file is the same picture the deck draws, produced by the same code.
#
#   bash scripts/waveform-png.sh track.flac /tmp/wave.png
#
# Exit codes: 0 ok, 1 build problem, 2 bad usage.
set -euo pipefail
cd "$(dirname "$0")/.."

if [ "$#" -ne 2 ]; then
  echo "usage: bash scripts/waveform-png.sh AUDIO OUT.png" >&2
  exit 2
fi

audio="$1"
out="$2"
if [ ! -f "$audio" ]; then
  echo "waveform-png.sh: no such file: $audio" >&2
  exit 2
fi

if ! command -v cargo >/dev/null 2>&1; then
  echo "waveform-png.sh: cargo is not in PATH; run this inside the dev shell" >&2
  exit 1
fi

target_dir="${CARGO_TARGET_DIR:-$HOME/.cache/dj-platform-target}"
envelope="$(mktemp -t waveform-XXXXXX).wave"
trap 'rm -f "$envelope"' EXIT

cargo run --quiet --release -p dj-headless -- waveform --in "$audio" --out "$envelope"
# No display is needed or wanted: the render is a file, not a window.
QT_QPA_PLATFORM=offscreen bash scripts/ui.sh --waveform-png "$envelope,$out"
echo "waveform-png.sh: wrote $out"

#!/usr/bin/env bash
# Who turns the headphone volume and the cue/master proportion.
#
# The S4 MK3 has both as physical knobs and never reports where they stand. While the controller is in use the window
# must hand those two over — a second stage for the same job is a knob that lies (owner's instruction, 23.11). Two
# halves are checked here: the engine answers who owns the knobs, and it holds the software bus at unity when the answer
# is "the hardware". The window draws what the engine answers, so the answer is the thing to get right.
set -euo pipefail
cd "$(dirname "$0")/.."
target="${CARGO_TARGET_DIR:-$PWD/target}"
engine="$target/debug/dj-engine"
if [ ! -x "$engine" ]; then
  echo "cue-hardware-check: $engine is not built" >&2
  exit 1
fi

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
mkdir -p "$work/off/hauler-dj" "$work/on/hauler-dj" "$work/data" "$work/run"
chmod 700 "$work/run"
printf 'version = 1\n' > "$work/off/hauler-dj/config.toml"
printf 'version = 1\n\n[controller]\nenabled = true\ndriver = "s4mk3"\n' > "$work/on/hauler-dj/config.toml"

ask() {
  XDG_CONFIG_HOME="$1" XDG_DATA_HOME="$work/data" XDG_RUNTIME_DIR="$work/run" "$engine" --cue-hardware 2>/dev/null | tr -d '\n'
}

off="$(ask "$work/off")"
on="$(ask "$work/on")"
if [ "$off" != "window" ]; then
  echo "cue-hardware-check: with no controller the answer was '$off', not 'window'" >&2
  exit 1
fi
if [ "$on" != "hardware" ]; then
  echo "cue-hardware-check: with the S4 MK3 the answer was '$on', not 'hardware'" >&2
  exit 1
fi

# The engine must also hold the bus at unity: the hardware does the level, and a software stage in series would
# silently cost decibels the DJ cannot see.
XDG_CONFIG_HOME="$work/on" XDG_DATA_HOME="$work/data" XDG_RUNTIME_DIR="$work/run" \
  timeout 30 "$engine" --null-sink --seconds 0.2 > "$work/run.log" 2>&1 || true
grep -q "the headphones' volume and mix are on the controller" "$work/run.log" || {
  echo "cue-hardware-check: the engine stayed silent about the headphones being the controller's" >&2
  exit 1
}

echo "cue-hardware-check: the window keeps the two knobs until a controller takes them; the software bus goes to unity"

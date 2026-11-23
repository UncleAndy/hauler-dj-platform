#!/usr/bin/env bash
# Where the engine sends its sound.
#
# The launcher cannot know a device name, so it starts the engine with an empty `--device` and expects the settings
# file to answer. That expectation was unmet until 2026-11-23: the empty name was read as "the system's own choice",
# so a controller chosen in the window made no sound unless the system default happened to be the same interface
# (owner's report — the interface was chosen in the app, the meters moved, the speakers were silent).
#
# The check asks the engine for a device that cannot exist and requires the log to name it: if the engine falls back to
# the system default instead, the name never appears and the check fails.
set -euo pipefail
cd "$(dirname "$0")/.."
target="${CARGO_TARGET_DIR:-$PWD/target}"
engine="$target/debug/dj-engine"
if [ ! -x "$engine" ]; then
  echo "device-choice-check: $engine is not built" >&2
  exit 1
fi

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
mkdir -p "$work/config/hauler-dj" "$work/data" "$work/run"
chmod 700 "$work/run"
printf 'version = 1\n\n[audio]\ndevice = "NoSuchDevice 123"\n' > "$work/config/hauler-dj/config.toml"

run() {
  XDG_CONFIG_HOME="$work/config" XDG_DATA_HOME="$work/data" XDG_RUNTIME_DIR="$work/run" \
    timeout 30 "$engine" "$@" --seconds 0.2 > "$work/out.log" 2>&1 || true
}

# 1. The launcher's placeholder plus a name in the settings file: the file decides.
run --device ""
grep -q 'the device "NoSuchDevice 123" would not open' "$work/out.log" || {
  echo "device-choice-check: an empty --device ignored the device named in the settings file" >&2
  grep -E 'output:|would not open' "$work/out.log" >&2 || true
  exit 1
}

# 2. No placeholder at all: the caller never asked for sound, so the engine stays silent.
run
grep -q 'output: null sink' "$work/out.log" || {
  echo "device-choice-check: an engine that was never asked for sound opened one anyway" >&2
  grep -E 'output:|would not open' "$work/out.log" >&2 || true
  exit 1
}

# 3. An explicit name beats the file: what the person typed just now wins.
run --device "Other Device 456"
grep -q 'the device "Other Device 456" would not open' "$work/out.log" || {
  echo "device-choice-check: an explicit --device did not beat the settings file" >&2
  grep -E 'output:|would not open' "$work/out.log" >&2 || true
  exit 1
}

echo "device-choice-check: the settings file chooses the output; silence stays silence; an explicit name wins"

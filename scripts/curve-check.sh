#!/usr/bin/env bash
# The crossfader law is the DJ's choice, not a session's accident (owner's request, 20.11).
#
# Two runs, two engines, one session file: the first run presses the "smooth" button through the same path a hand
# takes, and the session file has to carry the choice afterwards; the second run starts a fresh engine — with its
# own default law — presses nothing, and the snapshot has to report "smooth". A law that only lived in the window
# would be back at the default here, which is the defect this check exists for.
set -euo pipefail
cd "$(dirname "$0")/.."

repo="$(pwd)"
engine="${DJ_ENGINE:-$repo/target/release/dj-engine}"
ui="${DJ_UI:-${XDG_CACHE_HOME:-$HOME/.cache}/dj-platform-ui-build/apps/dj-ui/dj-ui}"

if [ ! -x "$engine" ]; then
    cargo build --release -p dj-engine >/dev/null
fi
if [ ! -x "$ui" ]; then
    bash scripts/ui.sh --build-only >/dev/null
fi

run_dir="$(mktemp -d)"
export XDG_DATA_HOME="$run_dir/data" XDG_CONFIG_HOME="$run_dir/config" XDG_RUNTIME_DIR="$run_dir/run"
mkdir -p "$XDG_DATA_HOME" "$XDG_CONFIG_HOME" "$XDG_RUNTIME_DIR"
export HAULER_IDLE_EXIT_MS=0
# The built-in detector keeps the run short: the law of the crossfader does not depend on how a track is analysed.
export HAULER_BEATMODEL=off

socket="$run_dir/curve.sock"
session="$run_dir/session.json"
pid=""

cleanup() {
    if [ -n "$pid" ]; then kill "$pid" 2>/dev/null || true; fi
    rm -rf "$run_dir"
}
trap cleanup EXIT

start_engine() {
    "$engine" --null-sink --socket "$socket" --block 256 >>"$run_dir/engine.log" 2>&1 &
    pid=$!
    for _ in $(seq 1 60); do [ -S "$socket" ] && break; sleep 0.1; done
    if [ ! -S "$socket" ]; then
        echo "curve-check: the engine did not open $socket" >&2
        tail -5 "$run_dir/engine.log" >&2 || true
        exit 1
    fi
}

stop_engine() {
    kill "$pid" 2>/dev/null || true
    wait "$pid" 2>/dev/null || true
    pid=""
    rm -f "$socket"
}

# First run: a hand chooses "smooth". `--curve-pick` presses the button a hand would press.
start_engine
QT_QPA_PLATFORM=offscreen "$ui" --connect "$socket" --session-file "$session" \
    --layout-file "$run_dir/layout.json" --curve-pick smooth \
    >"$run_dir/first.log" 2>&1 || { tail -6 "$run_dir/first.log" >&2; exit 1; }

if ! grep -q '"crossfader_curve": *"smooth"' "$session"; then
    echo "curve-check: the first run did not keep the law in the session file" >&2
    cat "$session" >&2 || true
    exit 1
fi

stop_engine

# Second run: a fresh engine, the same session, no clicks.
start_engine
QT_QPA_PLATFORM=offscreen "$ui" --connect "$socket" --session-file "$session" \
    --layout-file "$run_dir/layout.json" --curve-persist-check \
    >"$run_dir/second.log" 2>&1 || { tail -10 "$run_dir/second.log" >&2; exit 1; }

if ! grep -q "the crossfader law survived the restart: smooth" "$run_dir/second.log"; then
    echo "curve-check: the second run did not hand the law back" >&2
    grep -iE "crossfader" "$run_dir/second.log" | head -3 >&2 || true
    exit 1
fi

echo "curve-check: the law outlived the restart — smooth, chosen once, played again"

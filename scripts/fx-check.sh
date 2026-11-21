#!/usr/bin/env bash
# The unit in a deck's effect slot is a habit, not a session's accident (owner's decision, 21.11).
#
# Two runs, two engines, one session file: the first run presses "echo" in deck A's chooser through the path a hand
# takes, and the session file has to carry it; the second run starts a fresh engine — with its own default unit —
# presses nothing, and the snapshot has to report echo.
set -euo pipefail
cd "$(dirname "$0")/.."

repo="$(pwd)"
# The engine lives where the shell puts it (CARGO_TARGET_DIR from the dev shell), not in the repository:
# a stale  next to the sources answers with an old protocol and the check blames the engine.
engine="${DJ_ENGINE:-${CARGO_TARGET_DIR:-$HOME/.cache/dj-platform-target}/release/dj-engine}"
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
# The built-in detector keeps the run short: the unit does not depend on how a track is analysed.
export HAULER_BEATMODEL=off

socket="$run_dir/fx.sock"
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
        echo "fx-check: the engine did not open $socket" >&2
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

# First run: a hand chooses echo for deck A.
start_engine
QT_QPA_PLATFORM=offscreen "$ui" --connect "$socket" --session-file "$session" \
    --layout-file "$run_dir/layout.json" --fx-pick echo \
    >"$run_dir/first.log" 2>&1 || { tail -6 "$run_dir/first.log" >&2; exit 1; }

if ! grep -q '"deck_fx"' "$session"; then
    echo "fx-check: the first run did not keep the unit in the session file" >&2
    cat "$session" >&2 || true
    exit 1
fi
if ! grep -q '"echo"' "$session"; then
    echo "fx-check: the session file does not name echo" >&2
    cat "$session" >&2 || true
    exit 1
fi

stop_engine

# Second run: a fresh engine, the same session, no clicks.
start_engine
QT_QPA_PLATFORM=offscreen "$ui" --connect "$socket" --session-file "$session" \
    --layout-file "$run_dir/layout.json" --fx-persist-check \
    >"$run_dir/second.log" 2>&1 || { tail -10 "$run_dir/second.log" >&2; exit 1; }

if ! grep -q "the effect of deck A survived the restart: echo" "$run_dir/second.log"; then
    echo "fx-check: the second run did not hand the unit back" >&2
    grep -iE "fx|effect" "$run_dir/second.log" | head -3 >&2 || true
    exit 1
fi

echo "fx-check: the unit outlived the restart — echo, chosen once, played again"

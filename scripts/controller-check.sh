#!/usr/bin/env bash
# Controller support is off unless the DJ asks for it (owner's rule, 22.11).
#
# The rule has three halves, and all three are checked here against the real binaries: a fresh settings file has the
# feature off; the service, asked to run, says so and leaves successfully without opening a device; and when the file
# turns it on the service goes looking. The last step turns it back off, because a check must not leave the machine
# in the state it was testing.
set -euo pipefail
cd "$(dirname "$0")/.."

repo="$(pwd)"
target="${CARGO_TARGET_DIR:-$repo/target}/release"
service="${DJ_CONTROLLER:-$target/dj-controller}"
engine="${DJ_ENGINE:-$target/dj-engine}"

if [ ! -x "$service" ]; then
    cargo build --release -p dj-controller >/dev/null
fi
if [ ! -x "$engine" ]; then
    cargo build --release -p dj-engine >/dev/null
fi

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
export XDG_CONFIG_HOME="$work/config"
export XDG_DATA_HOME="$work/data"
mkdir -p "$XDG_CONFIG_HOME" "$XDG_DATA_HOME"

fail() {
    printf 'controller-check: %s\n' "$1" >&2
    exit 1
}

# 1. A machine that has never seen a controller: the setting is off and the service does nothing at all.
value="$("$engine" --get-config controller.enabled 2>/dev/null || true)"
if [ -n "$value" ] && [ "$value" != "false" ]; then
    fail "a fresh settings file says controller.enabled=$value, and it has to be false"
fi
off="$("$service" --status)"
case "$off" in
    *"controller support is off"*) ;;
    *) fail "with nothing set, the service did not report itself as off: $off" ;;
esac

# 2. The switch is the only thing that decides: with it on, the service looks for a device and says what it found.
"$engine" --set-config controller.enabled=true >/dev/null
"$engine" --set-config-string controller.driver=s4mk3 >/dev/null
on="$("$service" --status)"
case "$on" in
    *"controller support is on"*) ;;
    *) fail "with the setting on, the service did not start looking: $on" ;;
esac
case "$on" in
    *"HID nodes"*) ;;
    *) fail "the service did not say how many nodes it found: $on" ;;
esac

# 3. And back off: the same binary stops doing anything again, without a restart dance.
"$engine" --set-config controller.enabled=false >/dev/null
back="$("$service" --status)"
case "$back" in
    *"controller support is off"*) ;;
    *) fail "with the setting off again, the service kept going: $back" ;;
esac

# 4. Nothing is sent to a device in any of this: the capture mode refuses before it opens a node, and says why.
capture="$("$service" --capture "$work/reports.txt" 2>&1 || true)"
case "$capture" in
    *"controller support is off"*) ;;
    *) fail "capture did not stop at the switch: $capture" ;;
esac
if [ -s "$work/reports.txt" ]; then
    fail "a capture file was written while controller support was off"
fi

# 5. The other half of the path, end to end without a device: a file of control events goes through the binding
#    table into a real engine, and the engine's own answers are the witness. This is what a controller adds bytes to —
#    nothing else in the chain is new when the hardware arrives.
"$engine" --set-config controller.enabled=true >/dev/null
engine_socket="$work/engine.sock"
"$engine" --null-sink --socket "$engine_socket" --block 256 >/dev/null 2>&1 &
replay_engine=$!
for _ in $(seq 1 100); do
    [ -S "$engine_socket" ] && break
    sleep 0.05
done
printf 'deck_b.sync=1\nwheel_left=1.5\n' >"$work/events.txt"
replayed="$("$service" --replay "$work/events.txt" --engine "$engine_socket" 2>&1 || true)"
kill "$replay_engine" 2>/dev/null || true
wait "$replay_engine" 2>/dev/null || true
case "$replayed" in
    *"deck_b.sync → Applied"*) ;;
    *) fail "the command never reached the engine: $replayed" ;;
esac
case "$replayed" in
    *"wheel_left=1.5 asks for nothing the engine knows"*) ;;
    *) fail "an event the table does not know was not said out loud: $replayed" ;;
esac

# 6. And with the switch off the same replay sends nothing at all.
"$engine" --set-config controller.enabled=false >/dev/null
quiet="$("$service" --replay "$work/events.txt" --engine "$engine_socket" 2>&1 || true)"
case "$quiet" in
    *"controller support is off"*) ;;
    *) fail "a replay ran while the controller was off: $quiet" ;;
esac

printf 'controller-check: off by default, on when asked, off again; commands reach the engine and events the table does not know are named\n'

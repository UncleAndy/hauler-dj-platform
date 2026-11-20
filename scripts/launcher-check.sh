#!/usr/bin/env bash
# Запускатор: движок поднимается им, не переживает его и не поднимается дважды.
#
#   scripts/launcher-check.sh            — все случаи
#   scripts/launcher-check.sh --engine   — только движок и lifeline
#
# Печатает по строке на случай и выходит нулём только если все они прошли.
set -u

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
target="${CARGO_TARGET_DIR:-$HOME/.cache/dj-platform-target}/debug"
engine="$target/dj-engine"
launcher="$target/dj-launcher"
for binary in "$engine" "$launcher"; do
  if [ ! -x "$binary" ]; then
    echo "launcher-check: no $binary: build it first (cargo build -p dj-engine -p dj-launcher)" >&2
    exit 1
  fi
done

run_dir="$(mktemp -d)"
cleanup() {
  rm -rf "$run_dir"
}
trap cleanup EXIT
export XDG_DATA_HOME="$run_dir/data"
export XDG_CONFIG_HOME="$run_dir/config"
export XDG_RUNTIME_DIR="$run_dir"
mkdir -p "$XDG_DATA_HOME" "$XDG_CONFIG_HOME"

failed=0
engines_before=$(pgrep -fc "dj-engine" 2>/dev/null || echo 0)

# 1. Запускатор поднимает движок и говорит, что тот готов.
out="$("$launcher" --engine-only --verbose --timeout-ms 15000 2>&1)"
code=$?
if [ "$code" -eq 0 ] && printf '%s' "$out" | grep -q "engine ready"; then
  echo "launcher-check: the engine came up"
else
  echo "launcher-check: FAILED — the launcher did not bring the engine up (exit $code)" >&2
  printf '%s\n' "$out" | tail -5 >&2
  failed=1
fi

# 2. Lifeline: после выхода запускатора движка нет. На Linux это PDEATHSIG, на Windows — выход по простою,
#    поэтому окно ожидания с запасом.
sleep 3
engines_after=$(pgrep -fc "dj-engine" 2>/dev/null || echo 0)
if [ "$engines_after" -le "$engines_before" ]; then
  echo "launcher-check: the engine did not outlive the launcher ($engines_before -> $engines_after)"
else
  echo "launcher-check: FAILED — the engine outlived the launcher ($engines_before -> $engines_after)" >&2
  failed=1
fi

if [ "${1:-}" = "--engine" ]; then
  exit "$failed"
fi

# 3. Второй запускатор не поднимает второй движок: он видит замок занятым и уходит.
"$launcher" --engine-only --hold-ms 2000 --verbose --timeout-ms 15000 > "$run_dir/first.log" 2>&1 &
first=$!
sleep 0.4
second_out="$("$launcher" --engine-only --verbose --timeout-ms 15000 2>&1)"
second_code=$?
wait "$first" 2>/dev/null
if printf '%s' "$second_out" | grep -q "another launcher holds it"; then
  echo "launcher-check: the second launcher found the first one and left the engine alone (exit $second_code)"
else
  echo "launcher-check: FAILED — the second launcher started its own engine" >&2
  printf '%s\n' "$second_out" | tail -5 >&2
  failed=1
fi

# 4. Выход по простою: движок уходит сам после того, как клиент отключился.
idle_socket="$run_dir/idle.sock"
HAULER_IDLE_EXIT_MS=600 "$engine" --null-sink --socket "$idle_socket" > "$run_dir/idle.out" 2>&1 &
idle_pid=$!
for _ in $(seq 1 100); do
  [ -S "$idle_socket" ] && break
  sleep 0.05
done
nix shell nixpkgs#python3 --command python3 - "$idle_socket" <<'PY'
import json, socket, struct, sys, time
connection = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
connection.connect(sys.argv[1])
payload = json.dumps({'Hello': {'protocol': 'command-v1'}}).encode()
connection.sendall(struct.pack('<I', len(payload)) + payload)
time.sleep(0.4)
connection.close()
PY
gone=0
for _ in $(seq 1 40); do
  if ! kill -0 "$idle_pid" 2>/dev/null; then
    gone=1
    break
  fi
  sleep 0.25
done
if [ "$gone" -eq 1 ]; then
  echo "launcher-check: the engine left by itself after its last client"
else
  echo "launcher-check: FAILED — the engine stayed after its last client" >&2
  kill "$idle_pid" 2>/dev/null
  failed=1
fi
wait "$idle_pid" 2>/dev/null

if [ "$failed" -eq 0 ]; then
  echo "launcher-check: all cases passed"
fi
exit "$failed"

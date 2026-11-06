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

# Build outside the tree by default: a cache created from another checkout path makes CMake refuse
# to proceed ("the source ... does not match the source ... used to generate cache"), and a
# repository-local build/ then belongs to whoever ran the build first.
build_dir="${BUILD_DIR:-${XDG_CACHE_HOME:-$HOME/.cache}/dj-platform-ui-build}"

# If someone points BUILD_DIR at a cache from a different checkout, throw the cache away rather
# than fail with a CMake error that says nothing about what to do.
if [ -f "$build_dir/CMakeCache.txt" ] \
   && ! grep -q "^CMAKE_HOME_DIRECTORY:INTERNAL=$PWD$" "$build_dir/CMakeCache.txt"; then
  echo "ui.sh: $build_dir was configured for another checkout; reconfiguring" >&2
  rm -rf "$build_dir/CMakeCache.txt" "$build_dir/CMakeFiles"
fi

cmake -S . -B "$build_dir" -G Ninja
cmake --build "$build_dir"

case "${1:-}" in
  --build-only) exit 0 ;;
  --smoke)
    QT_QPA_PLATFORM=offscreen "$build_dir/apps/dj-ui/dj-ui" --smoke
    # The strip repaints every frame while scrolling: a frame must fit well inside 60 Hz.
    QT_QPA_PLATFORM=offscreen "$build_dir/apps/dj-ui/dj-ui" --waveform-bench 60
    # The browser must open where the last track came from, and follow a new load.
    QT_QPA_PLATFORM=offscreen "$build_dir/apps/dj-ui/dj-ui" \
        --session-file "${TMPDIR:-/tmp}/dj-session-check.json" --browser-start-check
    # F11 fills the screen, and the shape survives a restart: a window that forgets it is a window
    # that starts in the wrong place every time.
    for shape in window maximized; do
        printf '{"folder":"","window":"%s"}' "$shape" > "${TMPDIR:-/tmp}/dj-window-check.json"
        QT_QPA_PLATFORM=offscreen "$build_dir/apps/dj-ui/dj-ui" \
            --session-file "${TMPDIR:-/tmp}/dj-window-check.json" --window-check
    done
    # A restart that lands in full screen must still know it came from a maximized window.
    printf '{"folder":"","window":"fullscreen","restore_to":"maximized"}' \
        > "${TMPDIR:-/tmp}/dj-window-check.json"
    QT_QPA_PLATFORM=offscreen "$build_dir/apps/dj-ui/dj-ui" \
        --session-file "${TMPDIR:-/tmp}/dj-window-check.json" --window-check
    # The browser must select a row with a click and act on it: a list you cannot pick from is a
    # picture of a list.
    QT_QPA_PLATFORM=offscreen "$build_dir/apps/dj-ui/dj-ui" --browser-check
    # The arrangement must be the one the settings describe: a layout setting the shell silently
    # ignores is worse than none.
    QT_QPA_PLATFORM=offscreen "$build_dir/apps/dj-ui/dj-ui" --layout traktor \
        --layout-file "${TMPDIR:-/tmp}/dj-layout-check.json" --layout-check
    # A light panel means the palette was lost; the shell has to stay dark for a booth.
    QT_QPA_PLATFORM=offscreen "$build_dir/apps/dj-ui/dj-ui" --ui-theme-check
    # A non-default colour exercises the binding from the command line through QML: a misspelled
    # property would warn here instead of silently keeping the default.
    QT_QPA_PLATFORM=offscreen "$build_dir/apps/dj-ui/dj-ui" --smoke \
      --waveform-outside-color "#30343c"
    QT_QPA_PLATFORM=offscreen "$build_dir/apps/dj-ui/dj-ui" --smoke --waveform-style plain
    # The load dialog must not depend on a desktop package: a missing GTK schema aborts the
    # process, and that only shows when a user clicks. Checked here, headless.
    QT_QPA_PLATFORM=offscreen "$build_dir/apps/dj-ui/dj-ui" --dialog-check
    # A colour that is not a colour is refused, rather than accepted and drawn as black.
    if QT_QPA_PLATFORM=offscreen "$build_dir/apps/dj-ui/dj-ui" --smoke \
         --waveform-outside-color "not-a-colour" >/dev/null 2>&1; then
      echo "ui.sh: an invalid waveform colour was accepted" >&2
      exit 1
    fi
    ;;
  --engine-check)
    # The A06 acceptance, without a display or a sound card: run the real engine on a null sink,
    # let the real UI press Play, and require the ack and the published state to agree.
    # Build the engine rather than demand it: `cargo test` does not always leave the binary behind,
    # and a missing artefact is not the operator's problem to solve by hand.
    if [ -z "${DJ_ENGINE_BIN:-}" ]; then
      if ! command -v cargo >/dev/null 2>&1; then
        echo "ui.sh: cargo is not in PATH; run this inside the dev shell" >&2
        exit 1
      fi
      cargo build --quiet -p dj-engine -p dj-headless
    fi
    engine_bin="${DJ_ENGINE_BIN:-${CARGO_TARGET_DIR:-$HOME/.cache/dj-platform-target}/debug/dj-engine}"
    if [ ! -x "$engine_bin" ]; then
      echo "ui.sh: dj-engine is not executable at $engine_bin" >&2
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
    # A real file for the deck: generated by our own renderer, at the engine's rate, so the check
    # covers loading and not only transport.
    headless_bin="${CARGO_TARGET_DIR:-$HOME/.cache/dj-platform-target}/debug/dj-headless"
    fixture="$run_dir/track.wav"
    second="$run_dir/track-b.wav"
    if [ -x "$headless_bin" ]; then
      "$headless_bin" sine --frames 144000 --rate 48000 --out "$fixture" >/dev/null
      # A different length for deck B: two decks must not share one envelope.
      "$headless_bin" sine --frames 288000 --rate 48000 --out "$second" >/dev/null
    fi
    if [ -f "$fixture" ] && [ -f "$second" ]; then
      QT_QPA_PLATFORM=offscreen "$build_dir/apps/dj-ui/dj-ui" --connect "$socket" --self-check \
        --load "$fixture" --load-b "$second"
    elif [ -f "$fixture" ]; then
      QT_QPA_PLATFORM=offscreen "$build_dir/apps/dj-ui/dj-ui" --connect "$socket" --self-check \
        --load "$fixture"
    else
      QT_QPA_PLATFORM=offscreen "$build_dir/apps/dj-ui/dj-ui" --connect "$socket" --self-check
    fi
    # Shutting down: the window closed while the first snapshots arrive. The socket's destructor used
    # to deliver disconnected() into a half-destroyed client and a scene that was already gone, which
    # glibc reported as "corrupted double-linked list". A single slow exit never caught it, so the
    # check closes early and often.
    for delay in 120 200 400 900; do
      for _ in 1 2 3; do
        QT_QPA_PLATFORM=offscreen "$build_dir/apps/dj-ui/dj-ui" --connect "$socket" \
            --fps 144 --exit-after "$delay" > "${TMPDIR:-/tmp}/dj-shutdown.log" 2>&1
        if grep -qE "corrupted|free\(\)|Aborted|double free|unaligned" \
                "${TMPDIR:-/tmp}/dj-shutdown.log"; then
          echo "ui.sh: the UI did not shut down cleanly after ${delay} ms" >&2
          cat "${TMPDIR:-/tmp}/dj-shutdown.log" >&2
          exit 1
        fi
      done
    done
    echo "dj-ui: the UI shuts down cleanly when the window closes right after the start"

    # The picture, end to end: the envelope the engine published must reach the strip that draws
    # it. A waveform that never arrives looks exactly like a track that never loaded.
    QT_QPA_PLATFORM=offscreen "$build_dir/apps/dj-ui/dj-ui" --connect "$socket" --strip-check \
      --load "$fixture"
    # The master knob and meter: the knob must move the published gain, the meter must see the
    # signal, and the knob must sit above a shorter meter in the middle of the mixer.
    QT_QPA_PLATFORM=offscreen "$build_dir/apps/dj-ui/dj-ui" --connect "$socket" --master-check \
        --load "$fixture"
    # Every crossfader law must reach the engine and come back in the snapshot, and the selector must
    # show the one in force — a selector that lights up on the click is a selector that lies.
    QT_QPA_PLATFORM=offscreen "$build_dir/apps/dj-ui/dj-ui" --connect "$socket" --curve-check
    # The wheel trims a channel, and the engine has to hear it.
    QT_QPA_PLATFORM=offscreen "$build_dir/apps/dj-ui/dj-ui" --connect "$socket" \
        --wheel-check
    # Space belongs to the browser, not to playback: it folds the browser and leaves the decks
    # alone.
    QT_QPA_PLATFORM=offscreen "$build_dir/apps/dj-ui/dj-ui" --connect "$socket" \
        --space-check --load "$fixture"
    # And the refusal must be visible: pressing the deck's button on a playing deck blinks it
    # red instead of doing nothing.
    QT_QPA_PLATFORM=offscreen "$build_dir/apps/dj-ui/dj-ui" --connect "$socket" \
        --refusal-check --load "$fixture"
    # A playing deck must refuse a load, and say so: cutting the track is the one mistake a DJ
    # never forgives.
    QT_QPA_PLATFORM=offscreen "$build_dir/apps/dj-ui/dj-ui" --connect "$socket" \
        --load-busy-check --load "$fixture" --load-b "$second"
    # The channel gain, end to end: the UI's command must reach the deck it names and come back
    # in the snapshot. A knob that turns but changes nothing is the failure this catches.
    QT_QPA_PLATFORM=offscreen "$build_dir/apps/dj-ui/dj-ui" --connect "$socket" --gain-check
    # Drum-like material must not paint one colour: a tone has no balance to show, a kick and a hat
    # do. The engine reuses its cache, so this is a second UI pass over the same runtime.
    drums="crates/dj-codec/tests/fixtures/audio/drums-8s-48000.wav"
    if [ -f "$drums" ]; then
      # A second engine for the second pass: reconnecting to the engine that has just been playing
      # a two-deck load raced the loader thread and the deck came up empty, which is a defect of
      # its own to look at later — this check is about colour, not about reconnection.
      second_socket="$run_dir/engine-colour.sock"
      "$engine_bin" --null-sink --socket "$second_socket" --block 256 &
      second_pid=$!
      for _ in $(seq 1 100); do
        [ -S "$second_socket" ] && break
        sleep 0.05
      done
      QT_QPA_PLATFORM=offscreen "$build_dir/apps/dj-ui/dj-ui" --connect "$second_socket" \
        --self-check --load "$PWD/$drums" --waveform-expect-spread
      kill "$second_pid" 2>/dev/null || true
      wait "$second_pid" 2>/dev/null || true
    fi
    ;;
  *)
    exec "$build_dir/apps/dj-ui/dj-ui" "$@"
    ;;
esac

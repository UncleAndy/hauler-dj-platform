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
    # The checks must neither inherit nor rewrite what the person has arranged: their own config tree,
    # thrown away with the shell. Without this a check opens the owner's layout and a check that
    # changes a size writes it there.
    smoke_dir="$(mktemp -d)"
    export XDG_CONFIG_HOME="$smoke_dir/config"
    trap 'rm -rf "$smoke_dir"' EXIT
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
    # Its own layout file, thrown away first: the check moves the rail, the text and the deck zoom, so
    # a gate run must neither rewrite the layout the person is working in nor inherit the numbers the
    # previous run wrote there.
    rm -f "${TMPDIR:-/tmp}/dj-browser-layout.json"
    QT_QPA_PLATFORM=offscreen "$build_dir/apps/dj-ui/dj-ui" --browser-check \
        --layout-file "${TMPDIR:-/tmp}/dj-browser-layout.json"
    # And the next start reads them back: the check above leaves rail, text and zoom in the file, this
    # run is a different process that must open the way the last one was left.
    QT_QPA_PLATFORM=offscreen "$build_dir/apps/dj-ui/dj-ui" --browser-restore-check \
        --layout-file "${TMPDIR:-/tmp}/dj-browser-layout.json"
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
    # Metadata has a mirror keyed by the file's fingerprint, so two copies of one fixture share their
    # beats and cues. The gate points that mirror at its own directory: a run must not inherit the grid
    # of the run before it, or of the check before it.
    export XDG_DATA_HOME="$run_dir/data"
    # Same reason as in --smoke: the shell's own settings belong to a person, not to a gate run.
    export XDG_CONFIG_HOME="$run_dir/config"
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
    # The key is measured first, on a deck nothing has played yet: a later check leaves deck A
    # playing, and the engine refuses a load into a playing deck as Busy — a defect of its own that
    # has nothing to do with the key and must not decide whether this one passes.
    # Its own file, not a copy: the store keeps a mirror keyed by the file's fingerprint, so a copy of
    # the fixture shares its metadata and the check would look for a sidecar nothing had to write.
    key_track="$run_dir/key-track.wav"
    if [ -x "$headless_bin" ]; then
      "$headless_bin" sine --frames 96000 --rate 48000 --out "$key_track" >/dev/null
    fi
    QT_QPA_PLATFORM=offscreen "$build_dir/apps/dj-ui/dj-ui" --connect "$socket" --key-check \
      --load "$key_track"
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
    # The transport, end to end: Play moves the playhead, Stop puts it back on the cue point, and the
    # hand can scrub the overview — three gestures the deck is asked for by name (DJP-DECK-002).
    pads_copy="$run_dir/pads-$(basename "$fixture")"
    cp "$fixture" "$pads_copy"
    QT_QPA_PLATFORM=offscreen "$build_dir/apps/dj-ui/dj-ui" --connect "$socket" \
        --transport-check --load "$fixture"
    # The eight pads: setting one stores the playhead, a name and a colour travel through the engine,
    # and shift-click clears it (DJP-DECK-003). The copy matters: a pad is metadata, and a gate run
    # must not edit the fixture the rest of the suite is measured against.
    QT_QPA_PLATFORM=offscreen "$build_dir/apps/dj-ui/dj-ui" --connect "$socket" \
        --pads-check --load "$pads_copy"
    # The Loop button: a soft start, a second press that closes and activates the loop, a third that
    # clears it, and shift on the second press cancelling the pair (DJP-DECK-004).
    QT_QPA_PLATFORM=offscreen "$build_dir/apps/dj-ui/dj-ui" --connect "$socket" \
        --loop-check --load "$pads_copy"
    # The user stories: loops, cue points, mode switching, saving and deleting, each one driven through
    # the widgets and compared with the engine's own state.
    QT_QPA_PLATFORM=offscreen "$build_dir/apps/dj-ui/dj-ui" --connect "$socket" \
        --scenario-check --load "$pads_copy"
    # A playing deck must refuse a load, and say so: cutting the track is the one mistake a DJ
    # never forgives.
    QT_QPA_PLATFORM=offscreen "$build_dir/apps/dj-ui/dj-ui" --connect "$socket" \
        --load-busy-check --load "$fixture" --load-b "$second"
    # The channel gain, end to end: the UI's command must reach the deck it names and come back
    # in the snapshot. A knob that turns but changes nothing is the failure this catches.
    QT_QPA_PLATFORM=offscreen "$build_dir/apps/dj-ui/dj-ui" --connect "$socket" --gain-check
    # The law of the level knobs, both ways round: the curved law has to be soft around unity and steeper
    # at the end, and the straight law has to be exactly the numbers the knobs had before the curve
    # existed — a switch nobody can verify is a switch that lies.
    QT_QPA_PLATFORM=offscreen "$build_dir/apps/dj-ui/dj-ui" --gain-curve cubic --gain-curve-check
    QT_QPA_PLATFORM=offscreen "$build_dir/apps/dj-ui/dj-ui" --gain-curve linear --gain-curve-check
    # The faders have their own shape and their own middle, and a setting nobody exercises is a setting
    # that can rot: the B-taper, the cubic and the straight law all get a turn.
    QT_QPA_PLATFORM=offscreen "$build_dir/apps/dj-ui/dj-ui" --fader-curve cubic \
        --fader-curve-middle 0.5 --gain-curve-check
    QT_QPA_PLATFORM=offscreen "$build_dir/apps/dj-ui/dj-ui" --fader-curve linear --gain-curve-check
    # The channel EQ, end to end: three knobs per channel must reach the deck they name, come back in
    # the snapshot already applied, and show the engine's number rather than a local model.
    # B02: the engine measures the tempo on load, the deck shows it, and the number is the one the
    # analysis produced — the check reads the deck's own label, not the snapshot behind it. The
    # fixture is a click track at a known tempo: a sine has no beats, and a check on it would only
    # prove that the detector refuses, which `cargo test` already covers.
    # A copy, never the fixture: the analysis stores the grid beside the file (DJP-MEDIA-006).
    tempo_file="$run_dir/grid-track.wav"
    cp crates/dj-codec/tests/fixtures/audio/drums-8s-48000.wav "$tempo_file"
    QT_QPA_PLATFORM=offscreen "$build_dir/apps/dj-ui/dj-ui" --connect "$socket" \
        --tempo-check --load "$tempo_file"
    # The same measurement, asked for by hand: the button must put a question between the click and the
    # work, the deck must say it is working, and the answer must replace the grid the load wrote — both
    # in the file and on the strip. To see the strip change, it first has to be wrong: the stored grid is
    # what the deck shows, so the file is given a number the measurement cannot return.
    sed -i -E 's/"bpm": [0-9.eE+-]+/"bpm": 100.0/' "$run_dir/.djmeta/grid-track.wav.json"
    QT_QPA_PLATFORM=offscreen "$build_dir/apps/dj-ui/dj-ui" --connect "$socket" \
        --tempo-recheck --load "$tempo_file"
    # The grid editor: the DJ's ear outranks the detector. Each edit must reach the deck *and* the file,
    # and the file must come back locked against the next automatic measurement.
    grid_file="$run_dir/grid-edit.wav"
    cp crates/dj-codec/tests/fixtures/audio/drums-8s-48000.wav "$grid_file"
    tap_file="$run_dir/tap-grid.wav"
    cp crates/dj-codec/tests/fixtures/audio/drums-8s-48000.wav "$tap_file"
    # The same music as a different file: one sample changed by a bit, so the fingerprint differs and
    # the tap check starts from no grid at all instead of the one the editor check just wrote.
    printf '\x01' | dd of="$tap_file" bs=1 seek=1044 conv=notrunc status=none
    QT_QPA_PLATFORM=offscreen "$build_dir/apps/dj-ui/dj-ui" --connect "$socket" \
        --grid-check --load "$grid_file"
    # A client that reconnects to the running engine never loads the track, so nothing tells the engine
    # its tempo base: the buttons have to work on the deck as they find it.
    QT_QPA_PLATFORM=offscreen "$build_dir/apps/dj-ui/dj-ui" --connect "$socket" --grid-reconnect-check
    # The grid has to be visible on the strip, not only in the snapshot: beat lines, a bar every fourth,
    # and a spacing that follows the tempo — the same file, so the second run starts from a locked grid.
    QT_QPA_PLATFORM=offscreen "$build_dir/apps/dj-ui/dj-ui" --connect "$socket" \
        --grid-lines-check --load "$grid_file"
    # Taps are beats, not just a tempo: the grid has to come out on the line through the tap moments,
    # and a grid put on the play head instead fails this by most of a beat.
    QT_QPA_PLATFORM=offscreen "$build_dir/apps/dj-ui/dj-ui" --connect "$socket" \
        --tap-check --load "$tap_file"
    # The metronome: the button has to reach the engine, the state has to come back and light the
    # button, and the click has to be played on the beats — a flag is not a click, so the engine's own
    # count is what the check reads. Its own copy of the fixture, so the grid it clicks is this track's.
    metronome_file="$run_dir/metronome-track.wav"
    cp crates/dj-codec/tests/fixtures/audio/drums-8s-48000.wav "$metronome_file"
    # The button between the arrows: the grid is put where the play head is, and the file keeps it.
    # A loop of the music: four beats of the grid, and the arrow moves it by a beat without shortening it.
    # Its own copy of the fixture: the checks share one engine and one deck, and a check that loads the
    # same file name as the next one leaves the next one reading the stale grid of the reload in flight.
    loop_file="$run_dir/loop-track.wav"
    cp crates/dj-codec/tests/fixtures/audio/drums-8s-48000.wav "$loop_file"
    QT_QPA_PLATFORM=offscreen "$build_dir/apps/dj-ui/dj-ui" --connect "$socket" \
        --loop-beats-check --load "$loop_file"
    # Jumps, the platter and slip: a jump lands on a beat of the grid, a held nudge reaches the deck
    # and lets go, and slip carries a virtual head the state can draw (DJP-DECK-006/007). Its own copy
    # again: the checks share one engine and one deck.
    jump_file="$run_dir/jump-track.wav"
    cp crates/dj-codec/tests/fixtures/audio/drums-8s-48000.wav "$jump_file"
    QT_QPA_PLATFORM=offscreen "$build_dir/apps/dj-ui/dj-ui" --connect "$socket" \
        --transport-extras-check --load "$jump_file"
    # Sync, end to end: the buttons reach the engine, the state comes back, and the phase meter stands
    # where the engine says the beats are (DJP-SYNC-001/002). Two decks, two copies of the same music.
    sync_a="$run_dir/sync-a.wav"
    sync_b="$run_dir/sync-b.wav"
    cp crates/dj-codec/tests/fixtures/audio/drums-8s-48000.wav "$sync_a"
    cp crates/dj-codec/tests/fixtures/audio/drums-8s-48000.wav "$sync_b"
    # Its own engine: this check loads two decks and plays them, and reconnecting to the engine that has
    # just been playing a two-deck load raced the loader thread — the deck came up without music, and the
    # follower had nothing to stand on. Same reason as the colour check below.
    sync_socket="$run_dir/engine-sync.sock"
    "$engine_bin" --null-sink --socket "$sync_socket" --block 256 &
    sync_pid=$!
    for _ in $(seq 1 100); do
      [ -S "$sync_socket" ] && break
      sleep 0.05
    done
    QT_QPA_PLATFORM=offscreen "$build_dir/apps/dj-ui/dj-ui" --connect "$sync_socket" \
        --sync-check --load "$sync_a" --load-b "$sync_b"
    kill "$sync_pid" 2>/dev/null || true
    wait "$sync_pid" 2>/dev/null || true
    # Four decks are a claim about the whole chain: four deck objects, four mixer channels, four strips on
    # screen. One engine of its own, because the check loads four copies of the same fixture.
    four_socket="$run_dir/engine-four.sock"
    "$engine_bin" --null-sink --socket "$four_socket" --block 256 &
    four_pid=$!
    for _ in $(seq 1 100); do
      [ -S "$four_socket" ] && break
      sleep 0.05
    done
    four_file="$run_dir/four-decks.wav"
    cp crates/dj-codec/tests/fixtures/audio/drums-8s-48000.wav "$four_file"
    QT_QPA_PLATFORM=offscreen "$build_dir/apps/dj-ui/dj-ui" --connect "$four_socket" \
        --four-deck-check --load "$four_file"
    kill "$four_pid" 2>/dev/null || true
    wait "$four_pid" 2>/dev/null || true
    # S4: ten minutes of two decks is the soak the hand runs; the gate keeps a minute of the same shape —
    # the same click tracks, the same hands on the master, the follower and the loop, a tenth of the time.
    soak_socket="$run_dir/engine-soak.sock"
    "$engine_bin" --null-sink --socket "$soak_socket" --block 256 &
    soak_pid=$!
    for _ in $(seq 1 100); do
      [ -S "$soak_socket" ] && break
      sleep 0.05
    done
    QT_QPA_PLATFORM=offscreen "$build_dir/apps/dj-ui/dj-ui" --connect "$soak_socket" \
        --sync-soak --soak-seconds 60
    kill "$soak_pid" 2>/dev/null || true
    wait "$soak_pid" 2>/dev/null || true
    QT_QPA_PLATFORM=offscreen "$build_dir/apps/dj-ui/dj-ui" --connect "$socket" \
        --grid-here-check --load "$metronome_file"
    QT_QPA_PLATFORM=offscreen "$build_dir/apps/dj-ui/dj-ui" --connect "$socket" \
        --metronome-check --load "$metronome_file"
    QT_QPA_PLATFORM=offscreen "$build_dir/apps/dj-ui/dj-ui" --connect "$socket" --eq-check
    # A double click is the way back to neutral on every mouse-driven control, and each control has its
    # own neutral: a gain or a band goes flat, a fader opens, the master stands at three quarters of its
    # level and the crossfader centres.
    QT_QPA_PLATFORM=offscreen "$build_dir/apps/dj-ui/dj-ui" --connect "$socket" --neutral-check
    # The pitch fader, end to end: the hand's drag has to reach the engine as a rate, and a double click
    # has to bring the deck back to the file's own speed.
    QT_QPA_PLATFORM=offscreen "$build_dir/apps/dj-ui/dj-ui" --connect "$socket" --pitch-check
    # A13: a cue and a loop are decisions about a file, so they must outlive the engine process. One
    # run stores them, a second engine reads them with no cue command of its own.
    QT_QPA_PLATFORM=offscreen "$build_dir/apps/dj-ui/dj-ui" --connect "$socket" \
        --cue-write-check --load "$fixture"
    cue_socket="$run_dir/engine-cue.sock"
    kill "$engine_pid" 2>/dev/null || true
    wait "$engine_pid" 2>/dev/null || true
    "$engine_bin" --null-sink --socket "$cue_socket" --block 256 &
    engine_pid=$!
    for _ in $(seq 1 100); do
      [ -S "$cue_socket" ] && break
      sleep 0.05
    done
    QT_QPA_PLATFORM=offscreen "$build_dir/apps/dj-ui/dj-ui" --connect "$cue_socket" \
        --cue-read-check --load "$fixture"

    # A13: the headless render is reproducible byte for byte, which is what makes a fixture a
    # fixture rather than "whatever the current build produced".
    headless_bin="${DJ_HEADLESS_BIN:-${CARGO_TARGET_DIR:-$HOME/.cache/dj-platform-target}/debug/dj-headless}"
    if [ -x "$headless_bin" ]; then
      "$headless_bin" sine --frames 48000 --rate 48000 --out "$run_dir/take-one.wav" >/dev/null
      "$headless_bin" sine --frames 48000 --rate 48000 --out "$run_dir/take-two.wav" >/dev/null
      first="$(sha256sum "$run_dir/take-one.wav" | cut -d' ' -f1)"
      second_hash="$(sha256sum "$run_dir/take-two.wav" | cut -d' ' -f1)"
      echo "dj-headless: two runs hash ${first:0:16} and ${second_hash:0:16}"
      if [ "$first" != "$second_hash" ]; then
        echo "ui.sh: the same headless run produced different bytes" >&2
        exit 1
      fi
    fi

    # Drum-like material must not paint one colour: a tone has no balance to show, a kick and a hat
    # do. The engine reuses its cache, so this is a second UI pass over the same runtime.
    drums="crates/dj-codec/tests/fixtures/audio/drums-8s-48000.wav"
    if [ -f "$drums" ]; then
      # Load a copy, never the fixture: the check stores a hotcue, and a cue is metadata that lives
      # next to the music, so this run used to rewrite the fixture's own .djmeta — a golden file
      # changed by a gate run, dirtying the tree and drifting away from the manifest.
      drums_copy="$run_dir/drums-8s-48000.wav"
      cp "$drums" "$drums_copy"
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
        --self-check --load "$drums_copy" --waveform-expect-spread
      kill "$second_pid" 2>/dev/null || true
      wait "$second_pid" 2>/dev/null || true
    fi
    ;;
  *)
    exec "$build_dir/apps/dj-ui/dj-ui" "$@"
    ;;
esac

# Hauler DJ 0.2.0

**The first build that drives a DJ controller.** The whole Native Instruments Traktor Kontrol S4 MK3 panel is wired:
transport, hotcue pads, loops, jogs, tempo faders, mixer, effects, the two screens and every light. Everything 0.1.0 did
is still here — two decks through our own engine, the window as a separate process, hotcues and loops in
`.djmeta/<file>.json`, three-band EQ, seven effects, headphones, the Windows build.

> **The controller is off by default**, like every feature that opens a device: with `[controller] enabled = false` no
> service starts and no HID node is opened. Turn it on in **Settings → Controller** (`enabled`, then `driver = "s4mk3"`);
> a restart is needed, and so is permission to write the panel's node — see *Requirements*.

## On the controller

- **Transport.** PLAY starts and stops, CUE jumps to the stored cue and sets it with a hold, SYNC follows the master and
  lets go, MASTER takes the lead and gives it up, REV plays backwards, FLX slips the sound away from the track's head.
  Each is a switch: it reads the deck, so a press never undoes what the DJ is not looking at.
- **Hotcues and loops.** Eight pads per deck, lit by slot: a stored cue, a pad that holds a loop (in the colour the screen
  draws a loop with), an empty slot. A pad with a loop jumps into it, an empty pad with an active loop takes that loop,
  and only a pad with nothing writes a cue.
- **Jogs.** Touch a platter and it scratches — the sound follows the hand and the transport is not disturbed; turn it
  without touching and the phase bends, harder the faster the hand moves. The ring under the platter carries a marker that
  turns once per revolution of a record while the deck plays, and goes dark on a stopped deck.
- **Tempo faders are relative.** Moving a fader moves the app's pitch *from where the app already has it*; the whole travel
  of the fader is worth ±16 %. Plug the panel in with a fader anywhere and the pitch does not jump. **Shift** releases the
  fader: the hand can bring it back to the middle without the pitch following.
- **The pitch-zero lamp** beside the fader lights while the deck runs at the track's own speed (within one percent) and is
  out otherwise; each of the two states has its own colour in Settings, and the rule can be inverted.
- **Mixer.** Channel faders ride the window's own law (not a straight line), the crossfader, the channel gain and EQ and
  the master knob all move what the window moves, and PFL puts the channel in the headphones — as a hold or as a switch,
  whichever the settings say.
- **Effects.** Keys 1–4 load a chosen effect each, in its own colour; the degree knob has **three** positions that mean
  something: straight up is the dry signal, and each side is a different character of the same block (a filter becomes low
  pass one way and high pass the other, an echo a short slapback one way and a long tail the other).
- **Knobs beside the screen.** MOVE jumps by a step, LOOP lays a loop of a size; Shift with the same knob sets that step
  or that size. Both numbers stand in the screen's corners.
- **The browser.** The encoder walks the list, Shift walks the left column (sections, playlists, folders), the press loads
  the row the cursor is on — the same row the window highlights.
- **Deck select.** Each side of the panel drives one deck: A or C on the left, B or D on the right, for the transport, the
  pads, the platters, the knobs and the screens alike. The mixer stays the mixer.
- **Two screens**, 320×240, five frames a second: the deck's letter (the deck that side is switched to), state, tempo and
  pitch, title and artist, elapsed and remaining, the key, the whole track as a waveform with the playhead and the loop,
  the jump step and the loop size in the corners, and — while loop mode is on — `LOOP` lit on the floor.
- **The end of a track warns.** In the last twenty seconds (a setting) the deck's frame in the window, the jog ring and the
  screen's own frame go red and flash; a loop or a pause ends the warning. The ring is lit by the panel itself.
- **Lights.** Every state has a colour in Settings, night mode lights the whole panel faintly, and on the way out the panel
  goes dark and the screens say goodbye with the emblem and the name.

## In the window

- **The deck's transport row folds away** while a controller is connected and driving: the same buttons are under the hand
  on the panel. The switch is in Settings, for a desk with both.
- **The browser opens on the Collection**, remembers the rail place, and the row the cursor shows is the row the panel's
  encoder would load.
- **The effects knob's centre is the dry signal** — a knob at rest leaves the deck alone.
- **The level-correction label** beside the clock shows the correction the file asked for, not the product with the
  auto-gain setting (a setting at zero used to leave the label reading −120 dB).
- **Scrubbing is quiet**: the hand leads the playhead without touching the transport.

## Requirements

- **Linux:** a 64-bit distribution with PipeWire or ALSA, a GPU driver (OpenGL), and for the controller — permission to
  write its node. The panel needs a `udev` rule that gives your user access to `/dev/hidraw*` (and to the device's USB
  node for the screens); without it the service says so and nothing is lit.
- **Windows:** 64-bit Windows 10 or newer and the Microsoft Visual C++ Redistributable. The controller path on Windows is
  built from the same code and reads HID through a pure-Rust backend, but **a real panel on Windows was not exercised for
  this release** — it was measured on Linux, on the owner's own S4 MK3.
- **The controller is optional.** With it off, 0.2.0 is the 0.1.0 experience plus the window's changes above.

## Known limits

- **Motors are never driven.** The panel reports torque and accepts it, and this build deliberately sends none: no
  motorised platters, no brake feel. That is a safety decision (ADR-0006) which is still open, not an oversight.
- **One controller, one driver.** The S4 MK3 is the only device with a mapping; there is no MIDI and no generic HID.
- **Settings that need a restart** still need one: the controller's switch and driver, the display rates, and the lamp
  colours are read when the service starts.
- **The Windows build is unsigned.** SmartScreen shows "More info → Run anyway".
- **The Linux packages were not installed on a live Debian or Fedora.** They are built and inspected here; the first real
  `dpkg -i` / `dnf install` is still ahead.
- **The AppImage was not run** — its runtime needs `/lib64/ld-linux-x86-64.so.2`, which NixOS does not have.
- **One UI check is flaky under load** (`--selection-check`, the browser cursor reaching the engine); the same code passes
  on a rerun. It is a known defect, not a claim about this build.

## Files

The release carries the same set as 0.1.0, built from this tree:

| file | for |
|---|---|
| `hauler-dj_0.2.0_amd64.deb` | Debian, Ubuntu and their relatives |
| `hauler-dj-0.2.0-1.x86_64.rpm` | Fedora, openSUSE and their relatives |
| `hauler-dj-0.2.0-x86_64.AppImage` | one file, any distribution — `chmod +x` and run |
| `hauler-dj-0.2.0-nixos-x86_64.tar.gz` | NixOS (the ELF interpreter points into `/nix/store`) |
| `hauler-dj-0.2.0-x64.msi` | Windows x86_64, installer |
| `hauler-dj-windows.zip` | Windows x86_64, unpack and run |
| `SHA256SUMS` | checksums of every file above |

Sizes and checksums are the assets themselves, not a table here that goes stale.

## Verified

- `bash scripts/check.sh` — all gates: fixtures, formatting, clippy with `-D warnings`, 160 test binaries (995 tests),
  doc traceability, and the shell's own checks against a real engine: **34 of 34** scenarios, 761 witness lines, the
  frame-rate measurement and the launcher's shutdown.
- The controller was measured on hardware, by the owner, feature by feature as it was built: presses, jogs and scratch,
  the ring, the relative faders and the Shift release, the pitch-zero lamp, the screens (waveform, deck switching, the
  warning, the farewell), the lights and the effects keys. The protocol facts behind them live in
  `docs/hardware/s4mk3/`, each marked as measured on the device or taken from the public description.
- The window's own checks cover the browser, playlists, the settings window, the grid editor, the effects and the frame
  rate; the release gate ran them against a real engine, not a mock.

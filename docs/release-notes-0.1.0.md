# Hauler DJ 0.1.0

The first build that can be handed to someone else. Two decks play through our own engine; the window is a separate
process that talks to the engine over a socket, so audio never waits for the interface.

> **This is the first release, and it is meant for plain use on a computer** — a keyboard, a mouse and the screen.
> It is **not adapted for DJ controllers**: nothing here listens to MIDI or HID, and no controller mapping ships with
> this build. The design for controllers is written down (ADR-0006); the code is not in this release.

## What you can do

- **Play two decks.** Load a track with the deck's Load button or its hotkey. A playing deck refuses a new track
  instead of cutting the one you are listening to.
- **Mix.** Channel strips with a gain pointer on the meter, a crossfader with four laws (linear, constant power,
  smoothstep and sharp) and a master knob with its own coloured meter. The chosen law is kept between sessions.
- **Scratch and seek.** Dragging the waveform scrubs audibly; the scrub mode is a setting, not a hidden gesture.
- **See the music.** Waveforms zoom up to 400×, the beat grid is drawn in beats and bars, and cue and loop markers sit
  on the strip where they belong.
- **Keep your work.** Eight hotcues per deck and sample-exact loops. Both are written next to the music in
  `.djmeta/<file name>.json`, so copying a folder takes the work with it, and rebuilding the library index brings every
  cue back.
- **Seven effects.** Flanger, phaser, echo, filter, bitcrusher, noise and roll. Each has an amount knob and an on/off
  button; the chosen effect is remembered in `session.json`.
- **Listen on headphones.** Monitoring goes to spare channels of the main device, or to a second device with its own
  clock: the second stream follows the first within ±2 % and keeps a 40 ms cushion, so two sound cards with different
  rates stay together.
- **Set it up in one window.** Sound, analysis, library, mixer, effects, interface, diagnostics and about. Every row
  either applies at once or says "after a restart" — there are no knobs the code does not read.
- **Run it on Windows.** A native Windows build: the launcher starts the engine, the engine talks to the window over a
  loopback socket, and logs are written under `%APPDATA%\hauler-dj\logs`.

## Files

| file | for |
|---|---|
| `hauler-dj_0.1.0_amd64.deb` (223 MB) | Debian, Ubuntu and their relatives |
| `hauler-dj-0.1.0-1.x86_64.rpm` (253 MB) | Fedora, openSUSE and their relatives |
| `hauler-dj-0.1.0-x86_64.AppImage` (185 MB) | one file, any distribution — `chmod +x` and run |
| `hauler-dj-0.1.0-nixos-x86_64.tar.gz` (25 MB) | NixOS (the ELF interpreter points into `/nix/store`; on NixOS use `nix run` instead) |
| `hauler-dj-0.1.0-x64.msi` (47 MB) | Windows x86_64, installer |
| `hauler-dj-windows.zip` (45 MB) | Windows x86_64, unpack and run |
| `SHA256SUMS` | checksums of every file above |

The three Linux packages carry their own runtime — Qt, its plugins and QML modules, the font stack and the glibc loader —
so they do not care which glibc, Qt or library versions your distribution ships. That is why they are large: about
1.4 GB installed.

## Requirements

- **Linux:** a 64-bit distribution with PipeWire or ALSA, and a working GPU driver (OpenGL). Nothing else — Qt and the
  rest are inside the package.
- **Windows:** 64-bit Windows 10 or newer, plus the Microsoft Visual C++ Redistributable (the ONNX Runtime build needs it;
  it is not bundled).

## Known limits

- **No DJ controller support.** The audio interface is used for sound, not as a control surface: nothing listens to MIDI
  or HID, and no controller mapping ships with this build. Mixing is done with the mouse and the keyboard.
- **The Windows build is unsigned.** SmartScreen shows "More info → Run anyway".
- **The Linux packages were not installed on a live Debian or Fedora.** They were built and inspected here (`dpkg-deb -I`,
  `rpm -qip`, file lists, self-checks below), but the first `dpkg -i` / `dnf install` on a real machine is still ahead.
- **The AppImage was not run.** Its runtime is an ordinary ELF that needs `/lib64/ld-linux-x86-64.so.2`, which NixOS does
  not have; the image itself was assembled and its contents verified.
- **The second sound card path is covered by unit tests, not by a measurement on real hardware.** The tests feed the
  second stream at 48 kHz into a 44.1 kHz consumer, add 200 ppm of drift, and check that the ring holds; a live two-card
  session has not been recorded here.
- **Three-band EQ is not in this build.** It is the next step (A14).
- **One gate is flaky.** `--scroll-check` occasionally reports "the remembered row -1 is not in the middle" in the
  headless UI run; the same code passes on a rerun. It is a known defect, not a claim about this build.

## Verified

- `bash scripts/check.sh` — all gates: fixtures, formatting, clippy with `-D warnings`, the whole test suite, doc
  traceability, UI checks and the frame-rate measurement.
- The packaging runs its own checks rather than trusting the tools: every linked library resolves inside the bundle, the
  window starts from the bundle and loads all of its Qt plugins from it, the AppImage carries the squashfs signature at
  the runtime offset, and the deb and rpm report the right metadata and about 4200 files each.
- The Windows package was built from the same sources with mingw and reports 3 executables and 121 libraries with no
  dependencies outside the distribution.

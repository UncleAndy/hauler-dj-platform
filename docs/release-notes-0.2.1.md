# Hauler DJ 0.2.1

**The controller works on Windows.** 0.2.0 wired the whole Traktor Kontrol S4 MK3 panel, but only measured it on Linux;
this release carries what Windows needed — a pure-`hidapi` HID transport on every system, the screen's driver installed
by the installer instead of by hand, and three defects that only a real Windows machine showed. Linux gains the same
transport, quieter hotcue pads and a screens loop that keeps up with playback. Everything 0.2.0 could do is still here.

> **The controller is still off by default.** With `[controller] enabled = false` no service starts and no device is
> opened. Turn it on in **Settings → Controller**; a restart is needed.

## On the controller

- **Windows: the whole panel.** Buttons, knobs, jogs, faders and lights go through `hidapi` — one implementation for
  Linux, macOS and Windows — instead of our own `/dev/hidraw` reader. On Windows the vendored HID node is the one that
  speaks, and the service reads it the moment it starts.
- **Windows: the lights no longer die on a phantom refusal.** `hid_write` on Windows pads a short report up to the
  device's output length and returns the padded count — a 79-byte meter report comes back as 95. The service read that as
  "the panel did not take the report" and exited, taking the buttons and the screens with it. A padded write is a success
  now, and a refused report stops the lights alone: they stop, the presses and the screens keep running.
- **Windows: the screens get their driver.** The screens are a vendor interface that Windows hands to the vendor's driver;
  the distribution now carries a WinUSB package for it (`driver/`: `.inf`, `.cat`, and the certificate), the MSI binds it
  as part of the installation, and the restart Windows asks for is requested at the end of the install. The binding is
  forced (`UpdateDriverForPlugAndPlayDevices` with `INSTALLFLAG_FORCE`) because Windows will not replace a better-ranked
  driver with a package whose date is 2012, and unforced it left the interface where it was. A portable copy can do the
  same by hand: `dj-controller --screens-driver` (it asks for the prompt itself).
- **Unpainted hotcues wear the colour of their slot** — the same eight hues the window paints them with — so a stored pad
  is told from an empty one by colour and not by a two-step brightness difference nobody can see in a booth.
- **The screens keep up with playback.** The waveform strip's envelope was read from disk on every drawing, ten times a
  second; on Windows the loop fell behind the engine's snapshots and the picture froze on the state it had drawn first.
  The envelope is read once per track now.
- **The screens and the driver step say what they are doing**: the screens report snapshots, frames and the slowest frame
  every ten seconds, and every `--screens-driver` attempt is written to `logs/screens-driver.log`.
- **The pitch-zero lamp has its third state** (the fader is on its own zero point, not the track's), the pitch fader goes
  through the queue, and on the way out the level meters go dark with the buttons.

## In the window

- **Compatible keys are highlighted by the wheel**: the collection marks the tracks that mix with what is playing, by the
  Open Key circle.
- **A row says what the tempo does**: a sign for the direction instead of a coloured highlight, a flag when a track holds
  more than one tempo, and the tempo that takes up most of the track.
- **The rescan list holds still** while it fills, Cancel works, and the collection search accepts `~` against a number.

## On Windows packaging

- **The engine stops with the shell.**
- **A failed start names its cause**: the exception code, the age of the log it is reading, and the state of the connection
  log — instead of "the engine did not answer".
- **The MSI is written through `.part` and checked against its own `File` table**, and the runtime DLL set comes from the
  executables' import tables rather than a list written by hand.

## Requirements

- **Linux:** a 64-bit distribution with PipeWire or ALSA, a GPU driver (OpenGL), and for the controller — permission to
  write its node (a `udev` rule for `/dev/hidraw*` and for the device's USB node, which the screens need).
- **Windows:** 64-bit Windows 10 or newer and the Microsoft Visual C++ Redistributable. Administrator rights for the
  installation, because the screens' driver is bound during it, and a restart afterwards because Windows asks for one.
- **The controller is optional**, as before.

## Known limits

- **Motors are never driven** (ADR-0006 is still open). No motorised platters, no brake feel.
- **One controller, one driver.** The S4 MK3 is the only mapping; no MIDI, no generic HID.
- **Settings that need a restart** still need one.
- **The Windows build is unsigned.** SmartScreen shows "More info → Run anyway".
- **The Linux packages were not installed on a live Debian or Fedora**, and the AppImage was not run (its runtime needs
  `/lib64/ld-linux-x86-64.so.2`, which NixOS does not have).
- **One UI check is flaky under load** (`--selection-check`, the browser cursor reaching the engine); the same code passes
  on a rerun.

## Files

| file | for |
|---|---|
| `hauler-dj_0.2.1_amd64.deb` | Debian, Ubuntu and their relatives |
| `hauler-dj-0.2.1-1.x86_64.rpm` | Fedora, openSUSE and their relatives |
| `hauler-dj-0.2.1-x86_64.AppImage` | one file, any distribution — `chmod +x` and run |
| `hauler-dj-0.2.1-nixos-x86_64.tar.gz` | NixOS (the ELF interpreter points into `/nix/store`) |
| `hauler-dj-0.2.1-x64.msi` | Windows x86_64, installer — binds the screens' driver |
| `hauler-dj-windows.zip` | Windows x86_64, unpack and run (`dj-controller --screens-driver` for the screens) |
| `SHA256SUMS` | checksums of every file above |

Sizes and checksums are the assets themselves, not a table here that goes stale.

## Verified

- `bash scripts/check.sh` — all gates: fixtures, formatting, clippy with `-D warnings`, 161 test binaries (1023 tests),
  document traceability, and the shell's own checks against a real engine: **34 of 34** scenarios, 763 witness lines, the
  frame-rate measurement and the launcher's shutdown.
- **The controller was measured on Windows, by the owner, on his own S4 MK3** — this is the release where that happened:
  presses, faders, lights, the screens' driver installation, the screens drawing and, after the fix in this release,
  playing back with a running timer and a moving playhead.

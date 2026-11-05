# DJ Platform

An independent DJ platform for Linux/NixOS, built to reach practical functional parity with
Traktor Pro 4 through our own engine, our own UI and our own effects — no Traktor, Native
Instruments or Mixxx code, graphics, samples or firmware are used or copied.

**Status: A00, A01 and A02 done.** The specification, plan and backlog are approved (v0.1). The
repository holds the module skeleton, a pinned reproducible build environment, CI, and the first
piece of the audio engine: typed command contracts, a bounded RT queue, a sample-accurate scheduler
and a deterministic offline renderer. `dj-headless` renders a 440 Hz tone (and scripted command
scenarios) to WAV with an exact frame count; the UI is still a blank two-deck shell, and nothing
plays through a sound card yet — that is `dj-audio-io` (A05) and the engine process (A06).

| Area | State |
|---|---|
| Specification / plan / backlog | `DJ_Platform_TZ_v0.1_RU.md`, `DJ_Platform_Implementation_Plan_v0.1_RU.md`, `DJ_Platform_Initial_Backlog_v0.1.yaml` |
| Rust workspace | 13 crates: boundaries, owners and lint rules fixed |
| Engine core (A01) | `dj-domain` contracts, bounded queue, sample-accurate scheduler, offline render, RT guards |
| Media (A02) | `dj-codec`: WAV/FLAC/MP3/Vorbis decode, exact seek, bounded prefetch; PCM cache for lossy formats (ADR-0008) |
| Qt 6 / QML shell | blank two-deck window with a crossfader, starts headless (`--smoke`) |
| Build environment | `flake.nix` + `flake.lock` + `rust-toolchain.toml` (Rust 1.95.0, Qt 6.11.2) |
| CI | fmt, clippy `-D warnings`, tests, toolchain pin, QML smoke, requirement coverage |
| Audio backend, codecs, storage | not started — A02/A05/A08 |

## Architecture

```text
 Qt/QML UI  ──►  UI bridge / command gateway  ──►  bounded RT command queue
     ▲                                                      │
     │                                                      ▼
     │                                        audio engine (RT thread, fixed blocks)
     │                                                      │
     └──── read-only snapshot: revision, sample clock ◄──────┤
                                                            ├──► audio device adapter
 MIDI/HID controllers ──► controller service ──► gateway     │      (ALSA / PipeWire)
                                                            └──► recording ring ─► writer
 media workers (decode, prefetch) ──► PCM ring ──► engine
 analysis workers ──► waveform/cache ──► UI      storage workers ──► SQLite
```

The audio callback never allocates, locks, logs, touches disk/network, calls Qt or runs ML. State
changes reach the engine only as typed commands with an ack; the UI renders engine snapshots and
never treats its own optimistic state as confirmation. See `docs/architecture/rt-contract.md`.

## Stack

| Layer | Choice |
|---|---|
| Core, DSP, state | Rust stable 1.95.0, Cargo workspace, edition 2024 |
| UI | Qt 6.11.2 + QML + Qt Quick, separate from DSP |
| Bridge | separate UI process over a versioned local protocol (`command-v1`/`state-v1`), ADR-003 |
| Audio backend | CPAL vs native PipeWire/JACK — decision pending (ADR-002) |
| Decoders | Symphonia (WAV/FLAC first; MP3 after license review) |
| Storage | SQLite in WAL mode, versioned migrations |
| Build | Nix flake (NixOS 26.05), CMake + Ninja for the Qt part |

## Layout

```text
Cargo.toml            rust workspace (crates/*, apps/dj-headless, apps/dj-cli)
CMakeLists.txt        native build entry point (apps/dj-ui)
flake.nix             dev shell: rust, qt6, cmake, alsa/pipewire/openssl, bindgen config
scripts/check.sh      full local gate (rust + docs + UI smoke)
scripts/ui.sh         build and run the Qt/QML shell
apps/dj-ui            Qt/C++ shell + QML (not a cargo member)
apps/dj-headless      offline renderer / scenario runner (scaffold)
apps/dj-cli           collection and diagnostics CLI (scaffold)
crates/               13 domain crates, see docs/architecture/crate-map.md
schemas/              protocol JSON schemas (empty until A01)
docs/                 architecture, ADR, requirements, QA evidence, agent rules
assets/, benchmarks/  own graphics/audio only; measurement scripts and results
```

## Requirements

- Linux x86_64; the supported build host is NixOS 26.05 with flakes enabled.
- No network needed at runtime (DJP-ENV-004); Nix needs it for the first environment build.
- Wayland and X11 are separate test targets; the shell is verified with the offscreen platform in CI.

## Quickstart

```bash
nix develop                  # enters the pinned shell (rust 1.95.0, qt 6.11.2, cmake)
bash scripts/check.sh        # fmt, clippy, tests, toolchain pin, traceability, QML smoke
bash scripts/ui.sh           # build and open the blank two-deck window (needs a display)
bash scripts/ui.sh --smoke   # build, load QML offscreen, fail on any Qt warning
cargo run -p dj-audio-probe  # what CPAL sees on this machine (audio backend evidence, ADR-002)

# A01: render without a sound card and inspect the result
cargo run -p dj-headless -- sine --frames 48000 --measure --out /tmp/tone.wav
cargo run -p dj-headless -- scenario --file apps/dj-headless/tests/scenarios/play-stop.json
```

`dj-headless` prints a JSON report: exact frame count, peak/RMS, the sample at which each command
was applied, and — with `--measure` — p50/p99/max of the callback. Scenario files are JSON
(`{"frames": 4800, "commands": [{"at": 1000, "command": {"Play": {"deck": "A", "enabled": true}}}]}`);
the format is documented in `apps/dj-headless/src/scenario.rs`.

`nix develop` exports `CARGO_TARGET_DIR` to `~/.cache/dj-platform-target` so build artifacts stay
out of the checkout; `BUILD_DIR=/tmp/dj-ui-build bash scripts/ui.sh` moves the CMake tree out of it as
well (needed when the checkout is shared with another uid). `direnv allow` picks the shell up
automatically (`.envrc`).

On NixOS there is no `/usr/bin/env`, so invoke the scripts as `bash scripts/check.sh` — the
`#!/usr/bin/env bash` shebang is kept for the CI runners, where that path does exist.

## Documentation

- `docs/README.md` — document map and what is deliberately missing.
- `docs/architecture/crate-map.md` — crate ownership, planned crates, dependency rules.
- `docs/architecture/rt-contract.md` — the real-time rules and how they are verified.
- `docs/adr/` — license (0001), audio backend (0002), UI bridge (0003): all `accepted`.
- `docs/qa/evidence/A01/` — the deterministic render, the RT guards and the first block timings.
- `docs/qa/evidence/A02/` — decode speed, load-to-ready and seek measurements per format.
- `docs/requirements/traceability.csv` — every `DJP-*` requirement with an owning role.
- `docs/agents/AGENTS.md` — mandatory rules for AI agents working in this repository.
- `docs/qa/evidence/` — actual command output per task; `docs/qa/reference-rig.md` — test rig.

## License

Dual-licensed under **MIT OR Apache-2.0**, at your option: `LICENSE` (pointer), `LICENSE-MIT`,
`LICENSE-APACHE`; contributions are accepted under the same dual terms.

The policy is recorded in `docs/adr/0001-license.md` (accepted): GPL/AGPL dependencies are excluded
(so Encdr and Ableton Link are out), LGPL components (Qt, ALSA) only through dynamic linking with the
usual notices. Third-party inventory: `THIRD_PARTY.md`.

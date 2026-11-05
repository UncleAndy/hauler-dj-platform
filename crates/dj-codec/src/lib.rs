//! Media layer: probe, decode, indexed seek and bounded prefetch.
//!
//! Status: **A02**. Format support in R0 is WAV, FLAC, MP3 (CBR and VBR) and Ogg Vorbis; the
//! licence review for MP3 is closed (Symphonia, MPL-2.0; MP3 patents expired) — DJP-MEDIA-001.
//!
//! Two rules shape this crate:
//!
//! 1. **The decoder never runs on the audio thread.** [`prefetch::Prefetcher`] decodes in a worker
//!    thread into a bounded, allocation-free ring; the audio callback only copies from the ring
//!    (DJP-NFR-001, DJP-MEDIA-002).
//! 2. **The playback strategy is a property of the file, not of the moment** (ADR-0008): lossless
//!    formats stream with an index, lossy formats are decoded into the PCM cache at load.
//!
//! Ownership: media (`docs/architecture/crate-map.md`).
#![cfg_attr(not(test), deny(clippy::unwrap_used))]

pub mod decode;
pub mod info;
pub mod prefetch;

pub use decode::{DecodeStats, StreamDecoder};
pub use info::{GaplessInfo, MediaError, MediaFormat, MediaInfo};
pub use prefetch::{PrefetchConfig, PrefetchStats, Prefetcher, SeekOutcome};

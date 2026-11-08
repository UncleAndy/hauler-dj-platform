//! Waveform peak pyramid and its cache (A07, DJP-MEDIA-003).
//!
//! The waveform is a full decode of the file, so it is built off the audio path, stored next to the
//! collection and invalidated when the file's fingerprint changes. The timebase is source frames:
//! bucket `n` is frames `[n * frames_per_bucket, (n + 1) * frames_per_bucket)`, so the renderer
//! needs no scaling fudge to put the playhead in the right place.
//!
//! - Backlog task: A07
//! - Requirements: DJP-MEDIA-003, DJP-UI-004
#![cfg_attr(not(test), deny(clippy::unwrap_used))]

pub mod cache;
pub mod loudness;
pub mod tempo;
pub mod waveform;

pub use cache::{Fingerprint, WaveformCache, write_envelope};
pub use waveform::{Bucket, OVERVIEW_FRAMES_PER_BUCKET, Peaks, analyse};

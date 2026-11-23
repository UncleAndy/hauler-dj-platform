//! Writing reports to a device.
//!
//! The service used to be read-only on purpose, and the reason was good: nothing that can be told to a device should be
//! told to it by accident. Lights change that — an LED report is the only way to find out which byte lights which
//! control — so the permission is here, in one small place, with two implementations: a real node and a recorder that
//! keeps what would have been sent. The recorder is what makes this testable without a device, and it is what the
//! sweep's dry run uses.
//!
//! Motors do not go through here: they are forbidden until a separate decision (ADR-0006), and a forbidden thing has no
//! door in the code.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::Path;

/// Somewhere reports go.
pub trait ReportSink {
    /// Send one report. The bytes are the report as the driver sees it: the identifier first when the report is
    /// numbered, then the body.
    fn send(&mut self, report: &[u8]) -> Result<(), String>;
}

/// A node opened for writing.
pub struct NodeSink {
    file: File,
}

impl NodeSink {
    /// Open `path` for writing: the hidraw node of the device, which usually needs a `udev` rule to be writable by a
    /// person rather than by root.
    pub fn open(path: &Path) -> Result<Self, String> {
        let file = OpenOptions::new()
            .write(true)
            .open(path)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        Ok(Self { file })
    }
}

impl ReportSink for NodeSink {
    fn send(&mut self, report: &[u8]) -> Result<(), String> {
        self.file
            .write_all(report)
            .map_err(|error| format!("the node did not take the report: {error}"))?;
        self.file
            .flush()
            .map_err(|error| format!("the report did not leave: {error}"))
    }
}

/// A sink that keeps what it was given, so a test — or a dry run — can look at it.
#[derive(Default)]
pub struct RecordedSink {
    /// Every report, in the order it was sent.
    pub sent: Vec<Vec<u8>>,
}

impl ReportSink for RecordedSink {
    fn send(&mut self, report: &[u8]) -> Result<(), String> {
        self.sent.push(report.to_vec());
        Ok(())
    }
}

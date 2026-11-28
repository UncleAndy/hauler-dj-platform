//! Writing reports to a device.
//!
//! The service used to be read-only on purpose, and the reason was good: nothing that can be told to a device should be
//! told to it by accident. Lights change that — an LED report is the only way to find out which byte lights which
//! control — so the permission is here, in one small place, with two implementations: a real node and a recorder that
//! keeps what would have been sent. The recorder is what makes this testable without a device, and it is what the
//! sweep's dry run uses.
//!
//! The real node goes through `hidapi`, the same library that reads it (owner's decision, 2026-11-28): one backend for
//! every platform, and the bytes a report carries are the bytes the library takes — the report identifier first, then
//! the body.
//!
//! Motors do not go through here: they are forbidden until a separate decision (ADR-0006), and a forbidden thing has no
//! door in the code.

use crate::DeviceInfo;

/// Somewhere reports go.
pub trait ReportSink {
    /// Send one report. The bytes are the report as the driver sees it: the identifier first when the report is
    /// numbered, then the body.
    fn send(&mut self, report: &[u8]) -> Result<(), String>;
}

/// A node opened for writing.
pub struct HidSink {
    handle: hidapi::HidDevice,
}

impl HidSink {
    /// Open the node of `device` for writing.
    ///
    /// A HID node is written by the system's own rules: on Linux the hidraw node needs a `udev` rule to be writable by a
    /// person rather than by root, and the error here says which node and why, because "the lights do nothing" is a
    /// permission problem every second time.
    pub fn open(device: &DeviceInfo) -> Result<Self, String> {
        let api = hidapi::HidApi::new().map_err(|error| error.to_string())?;
        let path = device.hid_path()?;
        let handle = api
            .open_path(&path)
            .map_err(|error| format!("{}: {error}", device.path.display()))?;
        Ok(Self { handle })
    }
}

impl ReportSink for HidSink {
    fn send(&mut self, report: &[u8]) -> Result<(), String> {
        let sent = self
            .handle
            .write(report)
            .map_err(|error| format!("the node did not take the report: {error}"))?;
        if sent != report.len() {
            return Err(format!(
                "the node took {sent} of {} bytes of the report",
                report.len()
            ));
        }
        Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_recorder_keeps_every_report_in_order() {
        let mut sink = RecordedSink::default();
        assert!(sink.send(&[0x80, 1]).is_ok(), "a recorder always takes");
        assert!(sink.send(&[0x81, 2, 3]).is_ok(), "a recorder always takes");
        assert_eq!(sink.sent, vec![vec![0x80, 1], vec![0x81, 2, 3]]);
    }
}

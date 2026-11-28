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
        accepted(sent, report.len())
    }
}

/// Did the node take the whole report?
///
/// The count that comes back is **not** the length of the report on every platform: `hidapi`'s Windows backend pads a
/// short report up to the device's `OutputReportByteLength` before writing and returns the padded count (`hid.c`:
/// "Windows expects the number of bytes which are in the _longest_ report (plus one for the report number)"). On the
/// owner's panel that is 95: a 94-byte light report comes back as 95, and a 79-byte meter report also as 95 — and a
/// check that demanded equality turned every light report into "the panel did not take the meter report" while the
/// bytes had in fact gone out (owner's Windows report, 28.11). A **short** write is still a failure: that is the case
/// where part of the report was dropped.
fn accepted(sent: usize, wanted: usize) -> Result<(), String> {
    if sent < wanted {
        return Err(format!(
            "the node took {sent} of {wanted} bytes of the report"
        ));
    }
    Ok(())
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

    /// Windows answers with the padded length; a report that was taken in full must not be reported as a failure, and a
    /// report that lost bytes must be.
    #[test]
    fn a_padded_write_is_a_success_and_a_short_one_is_not() {
        assert!(accepted(94, 94).is_ok());
        assert!(
            accepted(95, 94).is_ok(),
            "Windows pads to the longest report"
        );
        assert!(accepted(95, 79).is_ok(), "a short report is padded too");
        match accepted(78, 94) {
            Ok(()) => panic!("a short write loses bytes, yet it was accepted"),
            Err(refused) => assert!(refused.contains("took 78 of 94"), "{refused}"),
        }
    }

    #[test]
    fn the_recorder_keeps_every_report_in_order() {
        let mut sink = RecordedSink::default();
        assert!(sink.send(&[0x80, 1]).is_ok(), "a recorder always takes");
        assert!(sink.send(&[0x81, 2, 3]).is_ok(), "a recorder always takes");
        assert_eq!(sink.sent, vec![vec![0x80, 1], vec![0x81, 2, 3]]);
    }
}

//! HID transport: finding the operating system's HID nodes and reading their reports.
//!
//! The crate knows nothing about any particular controller. It turns a folder of system nodes into
//! [`DeviceInfo`] values and hands back a reader for one of them; a driver on top decides what the bytes mean
//! (ADR-0006).
//!
//! The Linux backend reads `/sys/class/hidraw` and `/dev/hidraw*` with `std` only — no libudev, no FFI, so the
//! workspace's `unsafe_code = "deny"` holds. Other platforms come later behind the same [`Transport`] seam; until
//! then [`enumerate`] finds nothing there, which is the honest answer and not a crash.
//!
//! - Backlog task: A09
//! - Requirements: DJP-CTRL-001, DJP-CTRL-004
#![cfg_attr(not(test), deny(clippy::unwrap_used))]

pub mod write;

use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};

/// What is known about a node before it is opened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceInfo {
    /// The node to open, for example `/dev/hidraw3`.
    pub path: PathBuf,
    /// USB vendor id, for example `0x17cc`.
    pub vendor: u16,
    /// USB product id, for example `0x1720`.
    pub product: u16,
    /// The name the kernel reports, for example `Traktor Kontrol S4 MK3`.
    pub name: String,
}

impl DeviceInfo {
    /// Whether this node is the given device.
    #[must_use]
    pub fn is(&self, vendor: u16, product: u16) -> bool {
        self.vendor == vendor && self.product == product
    }
}

/// One device's reports.
///
/// Send, because reading belongs on its own thread: a blocking read on a device that says nothing must not hold the
/// clock of whoever is listening.
pub trait ReportSource: Send {
    /// Reads one report into `buffer` and returns how many bytes arrived.
    ///
    /// Blocks until the device sends something; an error means the node is gone or was never readable.
    fn read_report(&mut self, buffer: &mut [u8]) -> io::Result<usize>;
}

/// Finding nodes and opening them.
pub trait Transport {
    /// Every HID node this system has, in a stable order.
    fn enumerate(&self) -> io::Result<Vec<DeviceInfo>>;
    /// A reader for one node, open for reading only.
    fn open(&self, device: &DeviceInfo) -> io::Result<Box<dyn ReportSource>>;
}

/// The Linux nodes: a symlink farm in `/sys`, the devices themselves in `/dev`.
#[derive(Debug, Clone)]
pub struct HidrawTransport {
    root: PathBuf,
}

impl HidrawTransport {
    /// The nodes of the running system.
    #[must_use]
    pub fn new() -> Self {
        Self::with_root("/sys/class/hidraw")
    }

    /// The nodes under another folder, which is how the tests run without a device.
    #[must_use]
    pub fn with_root(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }
}

impl Default for HidrawTransport {
    fn default() -> Self {
        Self::new()
    }
}

impl Transport for HidrawTransport {
    fn enumerate(&self) -> io::Result<Vec<DeviceInfo>> {
        let entries = match fs::read_dir(&self.root) {
            Ok(entries) => entries,
            // A system without HID has no folder rather than an error: nothing to offer, nothing broken.
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error),
        };
        let mut devices = Vec::new();
        for entry in entries {
            let entry = entry?;
            let uevent = match fs::read_to_string(entry.path().join("device").join("uevent")) {
                Ok(text) => text,
                // A node the kernel is still setting up has no uevent yet; it will be there on the next look.
                Err(_) => continue,
            };
            let Some((vendor, product, name)) = parse_uevent(&uevent) else {
                continue;
            };
            devices.push(DeviceInfo {
                path: Path::new("/dev").join(entry.file_name()),
                vendor,
                product,
                name,
            });
        }
        devices.sort_by(|left, right| left.path.cmp(&right.path));
        Ok(devices)
    }

    fn open(&self, device: &DeviceInfo) -> io::Result<Box<dyn ReportSource>> {
        // Opened for reading only, which is the whole of A09's safety rule: no packet leaves this crate's devices.
        Ok(Box::new(NodeFile {
            file: File::open(&device.path)?,
        }))
    }
}

/// A hidraw node as a file.
struct NodeFile {
    file: File,
}

impl ReportSource for NodeFile {
    fn read_report(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.file.read(buffer)
    }
}

/// Vendor and product out of a sysfs `uevent`, where they are written as `HID_ID=0003:000017CC:00001720`.
fn parse_uevent(text: &str) -> Option<(u16, u16, String)> {
    let mut vendor = None;
    let mut product = None;
    let mut name = String::new();
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("HID_ID=") {
            let mut parts = rest.split(':');
            let _bus = parts.next();
            let read =
                |text: Option<&str>| text.and_then(|t| u16::from_str_radix(t.trim(), 16).ok());
            if let (Some(vendor_id), Some(product_id)) = (read(parts.next()), read(parts.next())) {
                vendor = Some(vendor_id);
                product = Some(product_id);
            }
        } else if let Some(rest) = line.strip_prefix("HID_NAME=") {
            name = rest.trim().to_string();
        }
    }
    Some((vendor?, product?, name))
}

/// A transport that hands out reports it was given, so the whole path is testable without a device.
#[derive(Debug, Clone, Default)]
pub struct RecordedTransport {
    devices: Vec<DeviceInfo>,
    reports: Vec<Vec<u8>>,
}

impl RecordedTransport {
    /// A transport with these nodes and these reports, in the order they were recorded.
    #[must_use]
    pub fn new(devices: Vec<DeviceInfo>, reports: Vec<Vec<u8>>) -> Self {
        Self { devices, reports }
    }
}

impl Transport for RecordedTransport {
    fn enumerate(&self) -> io::Result<Vec<DeviceInfo>> {
        Ok(self.devices.clone())
    }

    fn open(&self, _device: &DeviceInfo) -> io::Result<Box<dyn ReportSource>> {
        Ok(Box::new(Recorded {
            reports: self.reports.clone(),
            next: 0,
        }))
    }
}

/// The reports of a [`RecordedTransport`], one per read.
struct Recorded {
    reports: Vec<Vec<u8>>,
    next: usize,
}

impl ReportSource for Recorded {
    fn read_report(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let Some(report) = self.reports.get(self.next) else {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "the recording ended",
            ));
        };
        if report.len() > buffer.len() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "a report of {} bytes does not fit {}",
                    report.len(),
                    buffer.len()
                ),
            ));
        }
        buffer[..report.len()].copy_from_slice(report);
        self.next += 1;
        Ok(report.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_uevent_names_the_device() {
        let text =
            "DRIVER=hid-generic\nHID_ID=0003:000017CC:00001720\nHID_NAME=Traktor Kontrol S4 MK3\n";
        assert_eq!(
            parse_uevent(text),
            Some((0x17cc, 0x1720, "Traktor Kontrol S4 MK3".to_string()))
        );
    }

    #[test]
    fn a_uevent_without_ids_is_not_a_device_we_can_name() {
        assert_eq!(parse_uevent("HID_NAME=Something\n"), None);
    }

    #[test]
    fn the_node_name_is_what_matches_the_device() {
        let device = DeviceInfo {
            path: PathBuf::from("/dev/hidraw3"),
            vendor: 0x17cc,
            product: 0x1720,
            name: "Traktor Kontrol S4 MK3".to_string(),
        };
        assert!(device.is(0x17cc, 0x1720));
        assert!(!device.is(0x17cc, 0x1700));
    }

    #[test]
    fn a_system_without_hid_has_no_devices_and_no_error() {
        let transport = HidrawTransport::with_root("/nonexistent/hidraw-for-tests");
        assert_eq!(transport.enumerate().unwrap_or_default().len(), 0);
    }

    #[test]
    fn the_recorded_transport_gives_back_what_it_was_given() {
        let device = DeviceInfo {
            path: PathBuf::from("/dev/hidraw9"),
            vendor: 0x17cc,
            product: 0x1720,
            name: "recorded".to_string(),
        };
        let transport = RecordedTransport::new(vec![device.clone()], vec![vec![3, 1, 2], vec![1]]);
        assert_eq!(
            transport.enumerate().unwrap_or_default(),
            vec![device.clone()]
        );
        let mut source = match transport.open(&device) {
            Ok(source) => source,
            Err(error) => panic!("a recorded node always opens: {error}"),
        };
        let mut buffer = [0_u8; 8];
        assert_eq!(source.read_report(&mut buffer).unwrap_or(0), 3);
        assert_eq!(&buffer[..3], &[3, 1, 2]);
        assert_eq!(source.read_report(&mut buffer).unwrap_or(0), 1);
        assert!(source.read_report(&mut buffer).is_err());
    }
}

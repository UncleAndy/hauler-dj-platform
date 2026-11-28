//! HID transport: finding the operating system's HID nodes and reading their reports.
//!
//! The crate knows nothing about any particular controller. It turns the system's HID nodes into [`DeviceInfo`] values
//! and hands back a reader for one of them; a driver on top decides what the bytes mean (ADR-0006).
//!
//! The backend is **`hidapi`**: one implementation for Linux, macOS and Windows (owner's decision, 2026-11-28). The Linux
//! build reads the same hidraw nodes it always did — `hidapi` is a C library, and its Linux backend is the hidraw one —
//! so the bytes on the wire do not change; what changes is that there is no longer a platform with no backend at all.
//! The C library brings its own `unsafe`; none of it is in this crate, so the workspace's `unsafe_code = "deny"` holds.
//!
//! A transport keeps no state: `hidapi`'s device list is a cache read when the object is built, and a service that opens
//! the panel after the DJ plugs it in must not be looking at a list from before. Every call builds a fresh context.
//!
//! - Backlog task: A09
//! - Requirements: DJP-CTRL-001, DJP-CTRL-004
#![cfg_attr(not(test), deny(clippy::unwrap_used))]

pub mod write;

use std::ffi::CString;
use std::io;
use std::path::PathBuf;

/// What is known about a node before it is opened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceInfo {
    /// The node as the system spells it, for the eye and for ordering: `/dev/hidraw3` on Linux, a device path on
    /// Windows. It is for people and for sort order — it is not what the node is opened with.
    pub path: PathBuf,
    /// The same node as the system gave it, byte for byte. This is the only spelling `hidapi` accepts back, and a
    /// lossy rendering of a path is not it.
    pub raw: Vec<u8>,
    /// USB vendor id, for example `0x17cc`.
    pub vendor: u16,
    /// USB product id, for example `0x1720`.
    pub product: u16,
    /// The name the system reports, for example `Traktor Kontrol S4 MK3`. Empty when it reports none.
    pub name: String,
    /// Which interface of the device this node is, as the system numbers them. A composite panel has several, and the
    /// one that carries the buttons is not necessarily the first.
    pub interface: i32,
}

impl DeviceInfo {
    /// A node described from the four facts the system gives: ids, the path it must be opened with, a name that may be
    /// missing, and the interface number.
    #[must_use]
    pub fn from_parts(
        vendor: u16,
        product: u16,
        raw: Vec<u8>,
        name: Option<&str>,
        interface: i32,
    ) -> Self {
        Self {
            path: PathBuf::from(String::from_utf8_lossy(&raw).into_owned()),
            raw,
            vendor,
            product,
            name: name.unwrap_or_default().to_owned(),
            interface,
        }
    }

    /// Whether this node is the given device.
    #[must_use]
    pub fn is(&self, vendor: u16, product: u16) -> bool {
        self.vendor == vendor && self.product == product
    }

    /// The node as `hidapi` takes it back, or the reason it cannot be spelled that way.
    pub fn hid_path(&self) -> Result<CString, String> {
        CString::new(self.raw.clone()).map_err(|_| {
            format!(
                "{}: the node's path carries a zero byte, so it cannot be opened",
                self.path.display()
            )
        })
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

/// The system's own HID nodes, through `hidapi`.
#[derive(Debug, Clone, Copy, Default)]
pub struct HidTransport;

impl HidTransport {
    /// A transport for the running system.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl Transport for HidTransport {
    fn enumerate(&self) -> io::Result<Vec<DeviceInfo>> {
        // A fresh context every time: `HidApi::new` scans the bus, and a cached list from before the panel was plugged
        // in is the difference between "the controller works" and "the controller is not mentioned".
        let api = hidapi::HidApi::new().map_err(hid_error)?;
        let mut devices: Vec<DeviceInfo> = api
            .device_list()
            .map(|node| {
                DeviceInfo::from_parts(
                    node.vendor_id(),
                    node.product_id(),
                    node.path().to_bytes().to_vec(),
                    node.product_string(),
                    node.interface_number(),
                )
            })
            .collect();
        devices.sort_by(|left, right| left.path.cmp(&right.path));
        Ok(devices)
    }

    fn open(&self, device: &DeviceInfo) -> io::Result<Box<dyn ReportSource>> {
        let api = hidapi::HidApi::new().map_err(hid_error)?;
        let path = device
            .hid_path()
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
        let handle = api.open_path(&path).map_err(hid_error)?;
        Ok(Box::new(HidReader { handle }))
    }
}

/// A node opened through `hidapi`.
struct HidReader {
    handle: hidapi::HidDevice,
}

impl ReportSource for HidReader {
    fn read_report(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        // No timeout: a control that is not touched says nothing for minutes, and a reader that returns "nothing" every
        // few hundred milliseconds would spend the night looping over a silent device.
        self.handle.read_timeout(buffer, -1).map_err(hid_error)
    }
}

/// The library's own error, as an `io::Error`: callers of this crate speak `io::Result` and nothing else.
fn hid_error(error: hidapi::HidError) -> io::Error {
    io::Error::other(error.to_string())
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
    fn a_node_is_named_by_the_system_and_opened_by_its_own_bytes() {
        // The panel of the reference rig: one of its interfaces, as hidapi reports it.
        let device = DeviceInfo::from_parts(
            0x17cc,
            0x1720,
            b"/dev/hidraw3".to_vec(),
            Some("Traktor Kontrol S4 MK3"),
            -1,
        );
        assert_eq!(device.path, PathBuf::from("/dev/hidraw3"));
        assert_eq!(device.name, "Traktor Kontrol S4 MK3");
        assert_eq!(
            device.hid_path().map(|path| path.to_bytes().to_vec()),
            Ok(b"/dev/hidraw3".to_vec()),
            "the path is handed back exactly as the system gave it"
        );
        assert!(device.is(0x17cc, 0x1720));
        assert!(!device.is(0x17cc, 0x1700), "the product matters too");
    }

    #[test]
    fn a_node_without_a_name_is_still_a_node() {
        // Windows does not always answer with a product string, and a service that drops such a node would drop the
        // panel on exactly the systems nobody can debug.
        let device =
            DeviceInfo::from_parts(0x17cc, 0x1720, b"\\\\?\\hid#vid_17cc".to_vec(), None, 3);
        assert_eq!(device.name, "");
        assert_eq!(device.interface, 3);
        assert!(device.hid_path().is_ok());
    }

    #[test]
    fn a_path_with_a_zero_byte_is_refused_with_a_reason() {
        let device = DeviceInfo::from_parts(0x17cc, 0x1720, b"/dev/hid\0raw3".to_vec(), None, 0);
        let error = match device.hid_path() {
            Ok(path) => panic!("a zero byte cannot be opened, yet it became {path:?}"),
            Err(error) => error,
        };
        assert!(error.contains("zero byte"), "{error}");
    }

    #[test]
    fn the_recorded_transport_gives_back_what_it_was_given() {
        let device = DeviceInfo::from_parts(
            0x17cc,
            0x1720,
            b"/dev/hidraw9".to_vec(),
            Some("recorded"),
            0,
        );
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

//! Getting a picture onto the panel: the bulk endpoint the two screens hang on.
//!
//! The displays are not HID: they are a bulk interface of their own (`interface 4`, endpoint OUT `0x03`, public source —
//! see `docs/hardware/s4mk3/protocol-notes.md`). A frame is a rectangle, not necessarily the whole screen, so the bytes
//! come from `screen_frame`; this module only puts them on the wire.
//!
//! Nothing here is guessed about permissions: opening the panel needs the device node writable, which on Linux is a
//! `udev` rule with `MODE` and on Windows the WinUSB driver for this interface. When that is missing the error says so,
//! rather than reporting a device that is not there.

use std::io::Write;

use nusb::MaybeFuture;

/// Vendor and product of the controller the screens belong to.
const VENDOR: u16 = 0x17cc;
const PRODUCT: u16 = 0x1720;
/// The interface the displays sit on, and the endpoint they are written to.
const INTERFACE: u8 = 4;
const ENDPOINT: u8 = 0x03;
/// Bytes per transfer. One USBFS URB carries at most sixteen kilobytes, so a frame goes out in pieces and the device
/// reassembles it from the length in the header.
const CHUNK: usize = 16 * 1024;

/// The panel's screens, opened for writing.
pub struct Panels {
    /// Kept alive for as long as the endpoint: releasing the interface would cancel its transfers.
    interface: nusb::Interface,
    writer: nusb::io::EndpointWrite<nusb::transfer::Bulk>,
}

impl Panels {
    /// Open the controller's screen endpoint.
    ///
    /// The error is written for the person who has to fix it: no device on the bus, a device that will not open (the
    /// permission case), or an interface that is not the one the protocol names.
    pub fn open() -> Result<Self, String> {
        let info = nusb::list_devices()
            .wait()
            .map_err(|error| format!("the USB bus cannot be listed: {error}"))?
            .find(|device| device.vendor_id() == VENDOR && device.product_id() == PRODUCT)
            .ok_or_else(|| "no Traktor Kontrol S4 MK3 on the bus".to_string())?;
        let device = info.open().wait().map_err(|error| {
            format!(
                "the panel cannot be opened: {error}. On Linux this is a udev rule with MODE on the device node, \
                 on Windows the WinUSB driver for interface {INTERFACE}"
            )
        })?;
        let interface = device
            .claim_interface(INTERFACE)
            .wait()
            .map_err(|error| format!("display interface {INTERFACE} cannot be claimed: {error}"))?;
        let endpoint = interface
            .endpoint::<nusb::transfer::Bulk, nusb::transfer::Out>(ENDPOINT)
            .map_err(|error| format!("endpoint {ENDPOINT:#04x} is not a bulk out: {error}"))?;
        let writer = endpoint.writer(CHUNK);
        Ok(Self { interface, writer })
    }

    /// Write one frame — the header, the pixels and the footer `screen_frame` built.
    pub fn send(&mut self, frame: &[u8]) -> Result<usize, String> {
        self.writer
            .write_all(frame)
            .map_err(|error| format!("the frame did not go out: {error}"))?;
        self.writer
            .flush()
            .map_err(|error| format!("the frame was not flushed: {error}"))?;
        Ok(frame.len())
    }

    /// The interface the screens are on, for a log line that a person can check against the protocol.
    pub fn interface_number(&self) -> u8 {
        self.interface.interface_number()
    }
}

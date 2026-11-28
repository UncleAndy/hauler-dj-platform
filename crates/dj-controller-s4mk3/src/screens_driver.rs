//! The Windows half of the screens: the interface has to be on WinUSB before `nusb` can claim it.
//!
//! The screens are not HID. They are a vendor bulk interface (`interface 4`), and Windows gives that interface to
//! whichever driver the device's vendor installed — which is why the service says "display interface 4 cannot be
//! claimed: incompatible driver is installed for this interface" on a machine where the buttons already work.
//!
//! Binding it means installing a WinUSB driver package once, as an administrator: Windows only loads packages whose
//! catalogue is signed. There are two ways to have that package, and this module takes either:
//!
//! - **`libwdi.dll` beside the programs** (owner's decision, 2026-11-28): the library writes the package — its WinUSB
//!   binaries and its signed catalogue — into a folder and installs it, including its signing certificate. Everything
//!   is loaded at run time with `libloading`, so the DLL is optional: a copy without it still runs, and the command says
//!   what is missing instead of failing to start.
//! - **a prepared package in `driver/`**: an `.inf` with its `.cat`, made once on a Windows machine
//!   (`docs/hardware/windows-install.md`, step 1) and installed here with `pnputil`, after its certificate — when the
//!   packager shipped one — goes into the two stores Windows checks.
//!
//! Everything a caller needs to decide is a plain file question, so `--check` answers without administrator rights; only
//! the install itself needs them. Linux refuses the command rather than pretending: a udev rule is the Linux answer, and
//! a rule is not a driver.
//!
//! Three details of the library are not in the header's signatures, only in its source (`libwdi.c`, `libwdi/pki.c`) —
//! each was a silent defect in the first version of this module, so each is written down where it is used below:
//! a device with a driver on it is **not listed** unless the listing is asked for everything, the inf name must carry
//! `.inf`, and the catalogue is signed **only when options are passed** to `wdi_prepare_driver`.
#![cfg_attr(not(test), deny(clippy::unwrap_used))]

use std::path::{Path, PathBuf};

/// The device the screens belong to.
pub const VENDOR: u16 = 0x17cc;
/// Its product id, as the panel reports it.
pub const PRODUCT: u16 = 0x1720;
/// The interface the displays sit on — `interface 3` next to it is the vendor HID collection, where the buttons and
/// knobs live, and a WinUSB binding there would take them away from `hidusb`.
pub const INTERFACE: u8 = 4;
/// The name the package is made under. `libwdi` refuses a name without the extension
/// (`wdi_prepare_driver`: "Inf name provided must have a '.inf' extension"), and `pnputil` is given the same file, so
/// the extension belongs to the constant rather than to a call that could forget it.
pub const INF_NAME: &str = "s4_mk3_display.inf";
/// Where a prepared package is looked for, beside the programs.
pub const PACKAGE_FOLDER: &str = "driver";
/// The two stores a self-signed driver certificate has to be in for Windows to accept a package that was made on
/// another machine. `Root` makes the signer a trusted root; `TrustedPublisher` is the store Windows actually consults
/// for driver packages.
pub const CERTIFICATE_STORES: [&str; 2] = ["Root", "TrustedPublisher"];

/// What this machine offers, decided without asking for rights.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Offer {
    /// `libwdi.dll` sits beside the programs: it can write and install the package itself.
    Library(PathBuf),
    /// A prepared package: the `.inf` to hand to `pnputil`.
    Package(PathBuf),
    /// Nothing usable is there; the text says what to do about it.
    Missing(String),
}

impl Offer {
    /// One line for a journal or a console, in the words of the thing that was found.
    #[must_use]
    pub fn describe(&self) -> String {
        match self {
            Self::Library(path) => format!("libwdi: {}", path.display()),
            Self::Package(path) => format!("driver package: {}", path.display()),
            Self::Missing(text) => text.clone(),
        }
    }
}

/// What `programs` (the folder the exes were started from) carries.
///
/// The library wins over a package: it can make a package for the *installed* hardware — right interface, right version
/// of Windows — where a folder prepared elsewhere may not fit this machine at all.
#[must_use]
pub fn find_offer(programs: &Path) -> Offer {
    let library = programs.join("libwdi.dll");
    if library.is_file() {
        return Offer::Library(library);
    }
    let folder = programs.join(PACKAGE_FOLDER);
    let Ok(entries) = std::fs::read_dir(&folder) else {
        return Offer::Missing(missing_text(&folder));
    };
    let mut found: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("inf"))
        })
        .collect();
    // Sorted: two packages in one folder must not make the answer depend on the order the filesystem hands them out.
    found.sort();
    match found.into_iter().next() {
        Some(path) => Offer::Package(path),
        None => Offer::Missing(format!(
            "{} holds no .inf — a package is an .inf with its .cat and binaries",
            folder.display()
        )),
    }
}

/// The certificate that makes a prepared package trusted on a machine that did not make it, when the packager put one
/// beside the `.inf`.
///
/// `libwdi` signs the catalogue with a self-signed certificate and adds that certificate to `Root` and
/// `TrustedPublisher` **on the machine where the package is made** (`SelfSignFile` in `libwdi/pki.c`), so a package
/// that travels carries a signature nobody on the other machine trusts. The `.cer` exported from the `.cat` closes that
/// gap — and it is used only when it is there, because installing a root certificate is not something to guess at.
#[must_use]
pub fn package_certificate(inf: &Path) -> Option<PathBuf> {
    let folder = inf.parent()?;
    let mut found: Vec<PathBuf> = std::fs::read_dir(folder)
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("cer"))
        })
        .collect();
    found.sort();
    found.into_iter().next()
}

fn missing_text(folder: &Path) -> String {
    format!(
        "no libwdi.dll beside the programs and no package in {}: the screens need WinUSB on interface {INTERFACE}, and \
         this copy carries neither — see docs/hardware/windows-install.md",
        folder.display()
    )
}

/// What a person should do when nothing is shipped, in the order that costs the least.
#[must_use]
pub fn instructions() -> String {
    format!(
        "either put libwdi.dll beside the programs, or a prepared package (an .inf with its .cat) into {PACKAGE_FOLDER}/, \
         or bind interface {INTERFACE} once by hand — Zadig, options, list all devices, interface {INTERFACE}, WinUSB. \
         The package for `pnputil` is made on a Windows machine, as administrator, with: wdi-simple -n \"Traktor S4 MK3 \
         Display\" -v 0x{VENDOR:04x} -p 0x{PRODUCT:04x} -i {INTERFACE} -f {INF_NAME} -t 0 -d {PACKAGE_FOLDER} --extract"
    )
}

/// The command that puts a prepared package into the driver store.
///
/// `/install` as well as `/add-driver`: a panel that is already plugged in has to be re-bound now, not at the next
/// reboot, and `/install` is what makes Windows do it while the device is running.
#[must_use]
pub fn pnputil_command(inf: &Path) -> Vec<String> {
    vec![
        String::from("pnputil.exe"),
        String::from("/add-driver"),
        inf.display().to_string(),
        String::from("/install"),
    ]
}

/// The command that trusts one certificate for driver packages.
#[must_use]
pub fn certutil_command(certificate: &Path, store: &str) -> Vec<String> {
    vec![
        String::from("certutil.exe"),
        String::from("-addstore"),
        String::from("-f"),
        store.to_string(),
        certificate.display().to_string(),
    ]
}

/// The hardware id the panel's screen interface answers to — the same string the `.inf` names and Windows shows under
/// the device's instance path.
#[must_use]
pub fn hardware_id() -> String {
    format!("USB\\VID_{VENDOR:04X}&PID_{PRODUCT:04X}&MI_{INTERFACE:02X}")
}

/// Bind the interface, by whichever way this copy carries.
///
/// Windows only: on any other system the answer is a udev rule, and pretending otherwise would be a lie told by a
/// command that cannot work.
#[cfg(windows)]
pub fn install(programs: &Path) -> Result<String, String> {
    match find_offer(programs) {
        Offer::Library(dll) => libwdi::install(&dll),
        Offer::Package(inf) => install_package(&inf),
        Offer::Missing(text) => Err(format!("{text}\n{}", instructions())),
    }
}

/// Put a prepared package into the driver store, trusting its certificate first when one is shipped.
#[cfg(windows)]
fn install_package(inf: &Path) -> Result<String, String> {
    let mut said: Vec<String> = Vec::new();
    if let Some(certificate) = package_certificate(inf) {
        for store in CERTIFICATE_STORES {
            let (program, arguments) = run_certutil(&certificate, store)?;
            said.push(format!(
                "{program} put {} into {store}: {}",
                certificate.display(),
                arguments.trim()
            ));
        }
    }
    let command = pnputil_command(inf);
    let (program, arguments) = command.split_first().ok_or("empty command")?;
    let output = std::process::Command::new(program)
        .args(arguments)
        .output()
        .map_err(|error| format!("{program} did not run: {error}"))?;
    let answer = oem_text(&output.stdout, &output.stderr);
    if output.status.success() {
        said.push(format!("{program} took {}: {answer}", inf.display()));
    } else {
        // Not fatal on its own: `pnputil` puts the package in the driver store and *then* says whether the device took
        // it, and Windows refuses to replace a better-ranked driver. The vendor's 2018 driver outranks WinUSB's 2012
        // `DriverVer`, so the answer was "the driver package is not the best match for the device" with the package
        // staged as `oemN.inf` (owner's Windows log, 28.11). The binding is forced below; the answer is kept for the
        // journal either way.
        said.push(format!("{program} refused {}: {answer}", inf.display()));
    }
    // Windows ranks drivers by date and version before it will replace one, and a package whose `DriverVer` is 2012
    // never wins against the vendor's; `UpdateDriverForPlugAndPlayDevices` with `INSTALLFLAG_FORCE` is the documented way
    // to say "this device, this package, now" (it is what `devcon update` does). Without it the interface stays on the
    // vendor's driver for ever, which reads as "the screens do not work" (owner's report, 28.11).
    let bound = bind::forced(inf).map_err(|error| {
        format!(
            "{}; the interface could not be bound: {error}",
            said.join("; ")
        )
    })?;
    said.push(bound);
    Ok(said.join("; "))
}

/// Run one `certutil -addstore` and keep what it answered.
#[cfg(windows)]
fn run_certutil(certificate: &Path, store: &str) -> Result<(String, String), String> {
    let command = certutil_command(certificate, store);
    let (program, arguments) = command.split_first().ok_or("empty command")?;
    let output = std::process::Command::new(program)
        .args(arguments)
        .output()
        .map_err(|error| format!("{program} did not run: {error}"))?;
    let answer = oem_text(&output.stdout, &output.stderr);
    if output.status.success() {
        Ok((program.clone(), answer))
    } else {
        Err(format!(
            "{program} refused {} for {store}: {answer}",
            certificate.display()
        ))
    }
}

/// The two Windows calls the package path needs beyond `libwdi`: binding a driver to a device by force, and reading a
/// console program's answer in the code page it was written in.
///
/// Declared here rather than pulled in as a crate: two calls, and the project takes the platform's own declarations
/// elsewhere too (`shell32` in `dj-controller`, `libwdi` above).
#[cfg(windows)]
#[allow(unsafe_code)]
mod bind {
    use std::ffi::c_void;
    use std::path::Path;

    use super::{INTERFACE, hardware_id};

    /// `INSTALLFLAG_FORCE | INSTALLFLAG_NONINTERACTIVE`: replace whatever driver the device has, and never ask a
    /// question — an installer step has no desktop to put a dialog on.
    const FLAGS: u32 = 0x0000_0001 | 0x0000_0004;
    /// `CP_OEMCP` — the code page `pnputil` and `certutil` write their answers in.
    const OEM_CODE_PAGE: u32 = 1;

    #[link(name = "newdev")]
    unsafe extern "system" {
        /// `UpdateDriverForPlugAndPlayDevicesW(hwnd, hardware_id, inf_path, flags, reboot_required)`.
        fn UpdateDriverForPlugAndPlayDevicesW(
            hwnd: *mut c_void,
            hardware_id: *const u16,
            inf_path: *const u16,
            flags: u32,
            reboot: *mut i32,
        ) -> i32;
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        /// The code page of the console this program was started from — `pnputil` answers in that one.
        fn GetConsoleOutputCP() -> u32;
        /// The system's OEM code page: what is left when there is no console at all.
        fn GetOEMCP() -> u32;
        fn MultiByteToWideChar(
            code_page: u32,
            flags: u32,
            bytes: *const u8,
            length: i32,
            out: *mut u16,
            out_length: i32,
        ) -> i32;
    }

    /// Replace the device's driver with the package in `inf`, whatever Windows thinks of its rank.
    pub fn forced(inf: &Path) -> Result<String, String> {
        let hwid = wide(&hardware_id());
        let path = wide(&inf.display().to_string());
        let mut reboot: i32 = 0;
        // SAFETY: both strings are null-terminated UTF-16 that outlive the call, `reboot` points at a live `i32`, and
        // the function reads nothing else and keeps nothing of this side.
        let answered = unsafe {
            UpdateDriverForPlugAndPlayDevicesW(
                std::ptr::null_mut(),
                hwid.as_ptr(),
                path.as_ptr(),
                FLAGS,
                &mut reboot,
            )
        };
        if answered == 0 {
            return Err(format!(
                "Windows refused to replace the driver of {}: {}",
                hardware_id(),
                std::io::Error::last_os_error()
            ));
        }
        Ok(if reboot != 0 {
            format!(
                "interface {INTERFACE} is on WinUSB; Windows wants a restart before the device moves: reboot, or \
                 unplug the panel and plug it back in"
            )
        } else {
            format!("interface {INTERFACE} is on WinUSB")
        })
    }

    /// What a console program answered, in the code page it wrote in.
    pub fn oem(stdout: &[u8], stderr: &[u8]) -> String {
        format!("{}{}", decode(stdout), decode(stderr))
            .trim()
            .to_string()
    }

    fn decode(bytes: &[u8]) -> String {
        if bytes.is_empty() {
            return String::new();
        }
        let length = i32::try_from(bytes.len()).unwrap_or(i32::MAX);
        // The console's own code page first: `pnputil` writes in that one, and `CP_OEMCP` was 437 on the owner's Russian
        // Windows while the program answered in 866 — every Russian word came out as `╤ыєцсэр` (log of 28.11).
        // SAFETY: both calls take no arguments and return the current code page as a plain number.
        let code_page = unsafe {
            match GetConsoleOutputCP() {
                0 => match GetOEMCP() {
                    0 => OEM_CODE_PAGE,
                    page => page,
                },
                page => page,
            }
        };
        // SAFETY: the slice is valid for `length` bytes, and a null output pointer with a zero size only asks how long
        // the converted string would be.
        let needed = unsafe {
            MultiByteToWideChar(
                code_page,
                0,
                bytes.as_ptr(),
                length,
                std::ptr::null_mut(),
                0,
            )
        };
        if needed <= 0 {
            return String::from_utf8_lossy(bytes).into_owned();
        }
        let mut buffer = vec![0u16; needed as usize];
        // SAFETY: the buffer is exactly the size the first call asked for, and the pointer is valid for that size.
        let written = unsafe {
            MultiByteToWideChar(
                code_page,
                0,
                bytes.as_ptr(),
                length,
                buffer.as_mut_ptr(),
                needed,
            )
        };
        if written <= 0 {
            return String::from_utf8_lossy(bytes).into_owned();
        }
        String::from_utf16_lossy(&buffer[..written as usize])
    }

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }
}

/// The answer of a console program, decoded from the code page Windows wrote it in — `pnputil` speaks Russian in
/// cp866, and reading those bytes as UTF-8 turned its refusal into question marks (owner's Windows log, 28.11).
#[cfg(windows)]
fn oem_text(stdout: &[u8], stderr: &[u8]) -> String {
    bind::oem(stdout, stderr)
}

/// The C declarations, copied from `libwdi.h` 1.5.1 rather than guessed.
///
/// They are compiled on every platform so their layout can be checked here, on a machine that has no Windows: the
/// comments carry the offsets a C compiler prints for the same declarations, and
/// [`tests::the_c_structures_have_the_layout_the_header_declares`] compares them with what Rust computes. A structure
/// that misses a field is a read of somebody else's memory, and this is the one part of the driver call that cannot be
/// tried here.
#[allow(unsafe_code)]
#[cfg_attr(not(windows), allow(dead_code))]
pub mod ffi {
    use std::ffi::{c_char, c_int, c_void};

    /// `WDI_WINUSB` — the only driver type this program can use: `nusb` speaks WinUSB and nothing else.
    pub const WDI_WINUSB: c_int = 0;

    /// `struct wdi_device_info`: 80 bytes; `next` 0, `vid` 8, `pid` 10, `is_composite` 12, `mi` 16, `desc` 24,
    /// `driver` 32, `device_id` 40, `hardware_id` 48, `compatible_id` 56, `upper_filter` 64, `driver_version` 72.
    #[repr(C)]
    pub struct DeviceInfo {
        pub next: *mut DeviceInfo,
        pub vid: u16,
        pub pid: u16,
        /// `BOOL` is an `int` — four bytes, which is why `mi` sits at 16 and not at 14.
        pub is_composite: c_int,
        pub mi: u8,
        pub desc: *const c_char,
        pub driver: *const c_char,
        pub device_id: *const c_char,
        pub hardware_id: *const c_char,
        pub compatible_id: *const c_char,
        pub upper_filter: *const c_char,
        pub driver_version: u64,
    }

    /// `struct wdi_options_create_list`: 12 bytes; `list_all` 0, `list_hubs` 4, `trim_whitespaces` 8.
    #[repr(C)]
    pub struct OptionsCreateList {
        pub list_all: c_int,
        pub list_hubs: c_int,
        pub trim_whitespaces: c_int,
    }

    impl OptionsCreateList {
        /// What `wdi-simple` passes, and the only useful choice for this command.
        ///
        /// `libwdi.c` lists a device only when `options->list_all` is set; with a null options pointer it returns
        /// **driverless devices only**. The screens' interface is the opposite kind of device — one the vendor's driver
        /// already claims — so a null pointer hides the one device this command exists for, and the answer reads as "is
        /// the panel plugged in?".
        #[must_use]
        pub fn everything() -> Self {
            Self {
                list_all: 1,
                list_hubs: 1,
                trim_whitespaces: 1,
            }
        }
    }

    /// `struct wdi_options_prepare_driver`: 48 bytes; `driver_type` 0, `vendor_name` 8, `device_guid` 16,
    /// `disable_cat` 24, `disable_signing` 28, `cert_subject` 32, `use_wcid_driver` 40, `external_inf` 44.
    #[repr(C)]
    pub struct OptionsPrepare {
        pub driver_type: c_int,
        pub vendor_name: *const c_char,
        pub device_guid: *const c_char,
        pub disable_cat: c_int,
        pub disable_signing: c_int,
        pub cert_subject: *const c_char,
        pub use_wcid_driver: c_int,
        pub external_inf: c_int,
    }

    impl OptionsPrepare {
        /// WinUSB, everything else default — and, above all, **not null**: `wdi_prepare_driver` self-signs the `.cat`
        /// only when it is given options (`options != NULL && !disable_signing` in `libwdi.c`), and Windows 10 without
        /// test signing refuses an unsigned package with `WDI_ERROR_UNSIGNED`.
        #[must_use]
        pub fn winusb() -> Self {
            Self {
                driver_type: WDI_WINUSB,
                vendor_name: std::ptr::null(),
                device_guid: std::ptr::null(),
                disable_cat: 0,
                disable_signing: 0,
                cert_subject: std::ptr::null(),
                use_wcid_driver: 0,
                external_inf: 0,
            }
        }
    }

    /// `wdi_create_list(list, options)`.
    pub type CreateList =
        unsafe extern "C" fn(*mut *mut DeviceInfo, *const OptionsCreateList) -> c_int;
    /// `wdi_destroy_list(list)`.
    pub type DestroyList = unsafe extern "C" fn(*mut DeviceInfo) -> c_int;
    /// `wdi_prepare_driver(device, path, inf_name, options)`.
    pub type Prepare = unsafe extern "C" fn(
        *mut DeviceInfo,
        *const c_char,
        *const c_char,
        *const OptionsPrepare,
    ) -> c_int;
    /// `wdi_install_driver(device, path, inf_name, options)`; a null options pointer is "standard mode".
    pub type Install =
        unsafe extern "C" fn(*mut DeviceInfo, *const c_char, *const c_char, *const c_void) -> c_int;
    /// `wdi_strerror(code)`.
    pub type StrError = unsafe extern "C" fn(c_int) -> *const c_char;
    /// `wdi_is_driver_supported(driver_type, driver_info)`; `driver_info` may be null (the library calls it that way
    /// itself), and the answer is a `BOOL`.
    pub type IsDriverSupported = unsafe extern "C" fn(c_int, *mut c_void) -> c_int;
}

/// The interface, through `libwdi.dll` loaded at run time.
///
/// Every call is checked, and a DLL whose version has moved on answers with an error code that `wdi_strerror` turns
/// into words. The loading itself is `unsafe` — that is what a dynamic library is — so the permission is asked for
/// here, once, around the smallest surface that can work: no callback, nothing kept after the call.
#[cfg(windows)]
#[allow(unsafe_code)]
mod libwdi {
    use std::ffi::{CString, c_int};
    use std::path::Path;

    use super::ffi::{
        self, DeviceInfo, Install, IsDriverSupported, OptionsCreateList, OptionsPrepare, Prepare,
        StrError,
    };
    use super::{INF_NAME, INTERFACE, PRODUCT, VENDOR};

    /// Install the package the library makes for this panel's screen interface.
    pub fn install(dll: &Path) -> Result<String, String> {
        // SAFETY: the library is loaded once and stays alive for every call below; every symbol type comes from
        // `libwdi.h`, every options structure from the same header, and each call is given the arguments `wdi-simple.c`
        // gives it.
        unsafe {
            let library = libloading::Library::new(dll)
                .map_err(|error| format!("{} cannot be loaded: {error}", dll.display()))?;
            let create_list: libloading::Symbol<ffi::CreateList> = library
                .get(b"wdi_create_list")
                .map_err(|error| format!("{dll:?} is not libwdi: {error}"))?;
            let destroy_list: libloading::Symbol<ffi::DestroyList> = library
                .get(b"wdi_destroy_list")
                .map_err(|error| format!("{dll:?} is not libwdi: {error}"))?;
            let prepare: libloading::Symbol<Prepare> = library
                .get(b"wdi_prepare_driver")
                .map_err(|error| format!("{dll:?} is not libwdi: {error}"))?;
            let install_driver: libloading::Symbol<Install> = library
                .get(b"wdi_install_driver")
                .map_err(|error| format!("{dll:?} is not libwdi: {error}"))?;
            let supported: libloading::Symbol<IsDriverSupported> = library
                .get(b"wdi_is_driver_supported")
                .map_err(|error| format!("{dll:?} is not libwdi: {error}"))?;
            let strerror = library.get::<StrError>(b"wdi_strerror").ok();

            // Which driver this copy of the library can actually write. Asked first, because the library does not
            // refuse an unsupported type: it falls back to the first driver it *does* carry (`libwdi.c`), and binding
            // libusb0 or libusbK to the screens leaves them as unopenable as before — only with a driver that `nusb`
            // cannot speak.
            if supported(ffi::WDI_WINUSB, std::ptr::null_mut()) == 0 {
                return Err(format!(
                    "{} carries no WinUSB driver — it was built without one, and the library would fall back to \
                     libusb0 or libusbK, which this program cannot open. Take libwdi.dll from a Zadig release, or \
                     build it with the WDK",
                    dll.display()
                ));
            }

            let mut list: *mut DeviceInfo = std::ptr::null_mut();
            let code = create_list(&mut list, &OptionsCreateList::everything());
            if code != 0 || list.is_null() {
                return Err(format!(
                    "the USB devices cannot be listed: {}",
                    words(&strerror, code)
                ));
            }
            let Some(device) = find_interface(list) else {
                destroy_list(list);
                return Err(format!(
                    "no Traktor Kontrol S4 MK3 interface {INTERFACE} on this machine: is the panel plugged in?"
                ));
            };

            let folder = std::env::temp_dir().join("hauler-dj-screens-driver");
            let made = std::fs::create_dir_all(&folder)
                .map_err(|error| format!("{} cannot be made: {error}", folder.display()));
            if let Err(error) = made {
                destroy_list(list);
                return Err(error);
            }
            let path = c_string(&folder);
            let name = c_string(Path::new(INF_NAME));
            let (path, name) = match (path, name) {
                (Ok(path), Ok(name)) => (path, name),
                (Err(error), _) | (_, Err(error)) => {
                    destroy_list(list);
                    return Err(error);
                }
            };
            let prepared = prepare(
                device,
                path.as_ptr(),
                name.as_ptr(),
                &OptionsPrepare::winusb(),
            );
            let installed = if prepared == 0 {
                install_driver(device, path.as_ptr(), name.as_ptr(), std::ptr::null())
            } else {
                prepared
            };
            destroy_list(list);
            if prepared != 0 {
                return Err(format!(
                    "the package was not written: {}",
                    words(&strerror, prepared)
                ));
            }
            if installed != 0 {
                return Err(format!(
                    "the package was written but not installed (a driver install needs administrator rights): {}",
                    words(&strerror, installed)
                ));
            }
            Ok(format!(
                "interface {INTERFACE} is on WinUSB: the screens can be opened (the package it installed from is \
                 still in {})",
                folder.display()
            ))
        }
    }

    /// The first node of our device on the screen interface, or of a device that is not composite at all.
    fn find_interface(list: *mut DeviceInfo) -> Option<*mut DeviceInfo> {
        let mut node = list;
        while !node.is_null() {
            // SAFETY: `node` walks a list the library allocated, and stops at its own null terminator.
            let device = unsafe { &*node };
            if device.vid == VENDOR
                && device.pid == PRODUCT
                && (device.is_composite == 0 || device.mi == INTERFACE)
            {
                return Some(node);
            }
            node = device.next;
        }
        None
    }

    fn c_string(path: &Path) -> Result<CString, String> {
        CString::new(path.display().to_string()).map_err(|_| {
            format!(
                "{} carries a zero byte and cannot be handed to the driver installer",
                path.display()
            )
        })
    }

    fn words(strerror: &Option<libloading::Symbol<'_, StrError>>, code: c_int) -> String {
        let Some(strerror) = strerror else {
            return format!("libwdi code {code}");
        };
        // SAFETY: the library returns a static string for the code it was given, and it is not kept after this call.
        let text = unsafe { strerror(code) };
        if text.is_null() {
            format!("libwdi code {code}")
        } else {
            // SAFETY: a null-terminated C string owned by the library.
            unsafe { std::ffi::CStr::from_ptr(text) }
                .to_string_lossy()
                .into_owned()
        }
    }
}

/// The arguments the elevated copy is started with: the same command, marked as elevated, plus where to write the
/// answer.
///
/// The marker exists so the copy Windows starts never asks for a prompt of its own — a command that re-elevates itself
/// on every run would ask the DJ for a password until one of the copies gave up.
#[must_use]
pub fn elevated_parameters(report: &Path) -> String {
    format!(
        "--screens-driver --elevated --report \"{}\"",
        report.display()
    )
}

/// Ask Windows for an elevated copy of this program, and wait until it has written its answer into `report`.
///
/// A driver install needs administrator rights. A console program that answers "access denied" leaves the DJ hunting
/// for an elevated prompt, so the command asks for the prompt itself (`runas`); the elevated copy is this same program
/// with [`elevated_parameters`], and its answer travels back through `report`, because the copy Windows starts has a
/// console of its own that closes with it. Nothing is installed without that prompt.
#[cfg(windows)]
#[allow(unsafe_code)]
pub fn elevate(report: &Path) -> Result<(), String> {
    let program = std::env::current_exe()
        .map_err(|error| format!("this program's own path cannot be found: {error}"))?;
    let verb = wide("runas");
    let file = wide(&program.display().to_string());
    let parameters = wide(&elevated_parameters(report));
    // SAFETY: four null-terminated UTF-16 strings built just above outlive the call; `ShellExecuteW` reads them and
    // nothing else, returns an `HINSTANCE`-shaped integer (32 or less means the shell refused), and no pointer comes
    // back to this side.
    let answer = unsafe {
        shell::ShellExecuteW(
            std::ptr::null_mut(),
            verb.as_ptr(),
            file.as_ptr(),
            parameters.as_ptr(),
            std::ptr::null(),
            shell::SW_SHOWNORMAL,
        )
    };
    if answer as usize <= 32 {
        return Err(format!(
            "Windows did not start the elevated copy (code {}): the prompt was cancelled, or this account cannot \
             elevate. Run `dj-controller --screens-driver` from a console started as administrator",
            answer as usize
        ));
    }
    // The child writes the report as its last act, so the file appearing with something in it is the end of the wait.
    // Five minutes covers a slow driver install and a prompt nobody answered yet; the message says what to do if it
    // runs out.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(300);
    while std::time::Instant::now() < deadline {
        if let Ok(text) = std::fs::read_to_string(report)
            && !text.trim().is_empty()
        {
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    Err(format!(
        "the elevated copy did not write {} in five minutes: answer the Windows prompt it opened, or run \
         `dj-controller --screens-driver` from a console started as administrator",
        report.display()
    ))
}

/// A null-terminated UTF-16 copy of `text`, which is what every `W` entry point of Windows takes.
#[cfg(windows)]
fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// The one Windows call outside `libwdi`: asking for an elevated copy of this program.
///
/// Declared here rather than pulled in as a binding crate: it is one call, and the project takes the platform's own
/// declarations over a crate everywhere else too. `HINSTANCE` and `HWND` are declared as `*mut c_void`, which is what
/// they are.
#[cfg(windows)]
#[allow(unsafe_code)]
mod shell {
    use std::ffi::{c_int, c_void};

    /// `SW_SHOWNORMAL` — the console of the elevated copy is what the DJ has to see.
    pub const SW_SHOWNORMAL: c_int = 1;

    #[link(name = "shell32")]
    unsafe extern "system" {
        pub fn ShellExecuteW(
            hwnd: *mut c_void,
            verb: *const u16,
            file: *const u16,
            parameters: *const u16,
            directory: *const u16,
            show: c_int,
        ) -> *mut c_void;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The crate refuses `expect` everywhere, tests included: a witness that panics with its own words is the same thing
    /// with one line of help.
    fn ok<T, E: std::fmt::Debug>(result: Result<T, E>) -> T {
        match result {
            Ok(value) => value,
            Err(error) => panic!("{error:?}"),
        }
    }

    #[test]
    fn a_copy_with_neither_carries_says_so_and_names_the_folder() {
        let folder = std::env::temp_dir().join(format!("dj-screens-none-{}", std::process::id()));
        ok(std::fs::create_dir_all(&folder));
        match find_offer(&folder) {
            Offer::Missing(text) => {
                assert!(text.contains("libwdi.dll"), "{text}");
                assert!(text.contains("windows-install.md"), "{text}");
            }
            other => panic!("nothing is there, yet {other:?} was found"),
        }
        std::fs::remove_dir_all(&folder).ok();
    }

    #[test]
    fn the_library_wins_over_a_prepared_package() {
        let folder = std::env::temp_dir().join(format!("dj-screens-both-{}", std::process::id()));
        let driver = folder.join(PACKAGE_FOLDER);
        ok(std::fs::create_dir_all(&driver));
        ok(std::fs::write(driver.join("panel.inf"), b""));
        ok(std::fs::write(folder.join("libwdi.dll"), b""));
        match find_offer(&folder) {
            Offer::Library(path) => assert_eq!(
                path.file_name().and_then(|name| name.to_str()),
                Some("libwdi.dll")
            ),
            other => panic!("the library should be preferred: {other:?}"),
        }
        std::fs::remove_dir_all(&folder).ok();
    }

    #[test]
    fn a_package_is_an_inf_and_two_of_them_do_not_depend_on_the_folder_order() {
        let folder = std::env::temp_dir().join(format!("dj-screens-inf-{}", std::process::id()));
        let driver = folder.join(PACKAGE_FOLDER);
        ok(std::fs::create_dir_all(&driver));
        ok(std::fs::write(driver.join("b_panel.inf"), b""));
        ok(std::fs::write(driver.join("a_panel.inf"), b""));
        ok(std::fs::write(driver.join("readme.txt"), b""));
        match find_offer(&folder) {
            Offer::Package(path) => assert_eq!(
                path.file_name().and_then(|name| name.to_str()),
                Some("a_panel.inf")
            ),
            other => panic!("a package is there: {other:?}"),
        }
        std::fs::remove_dir_all(&folder).ok();
    }

    #[test]
    fn the_install_line_rebinds_a_panel_that_is_already_plugged_in() {
        let command = pnputil_command(Path::new("C:\\Hauler DJ\\driver\\panel.inf"));
        assert_eq!(command.first().map(String::as_str), Some("pnputil.exe"));
        assert!(
            command.iter().any(|part| part == "/add-driver"),
            "{command:?}"
        );
        assert!(command.iter().any(|part| part == "/install"), "{command:?}");
        assert!(
            command.iter().any(|part| part.ends_with("panel.inf")),
            "{command:?}"
        );
    }

    #[test]
    fn the_instructions_name_the_interface_the_ids_and_the_driver() {
        let text = instructions();
        assert!(text.contains("interface 4"), "{text}");
        assert!(text.contains("0x17cc"), "{text}");
        assert!(text.contains("WinUSB"), "{text}");
        assert!(text.contains(INF_NAME), "{text}");
        // `wdi-simple` answers a bare name with "Inf name provided must have a '.inf' extension"; the line a person
        // copies has to carry the extension, and the interface has to be the screens' one, not the HID collection's.
        assert!(text.contains("s4_mk3_display.inf"), "{text}");
        assert!(text.contains("-i 4 "), "{text}");
        assert!(text.contains("--extract"), "{text}");
    }

    #[test]
    fn the_inf_name_the_library_is_given_has_the_extension_it_demands() {
        assert!(INF_NAME.ends_with(".inf"), "{INF_NAME}");
        assert!(!INF_NAME.contains(['/', '\\']), "{INF_NAME}");
    }

    #[test]
    fn a_certificate_beside_the_package_is_found_and_two_of_them_do_not_depend_on_the_folder_order()
    {
        let folder = std::env::temp_dir().join(format!("dj-screens-cer-{}", std::process::id()));
        let driver = folder.join(PACKAGE_FOLDER);
        ok(std::fs::create_dir_all(&driver));
        let inf = driver.join(INF_NAME);
        ok(std::fs::write(&inf, b""));
        assert_eq!(package_certificate(&inf), None);
        ok(std::fs::write(driver.join("b_driver.cer"), b""));
        ok(std::fs::write(driver.join("a_driver.cer"), b""));
        assert_eq!(package_certificate(&inf), Some(driver.join("a_driver.cer")));
        std::fs::remove_dir_all(&folder).ok();
    }

    #[test]
    fn the_certificate_goes_into_the_two_stores_windows_checks() {
        assert_eq!(CERTIFICATE_STORES, ["Root", "TrustedPublisher"]);
        let command = certutil_command(Path::new("C:\\Hauler DJ\\driver\\driver.cer"), "Root");
        assert_eq!(command.first().map(String::as_str), Some("certutil.exe"));
        assert!(
            command.iter().any(|part| part == "-addstore"),
            "{command:?}"
        );
        assert!(command.iter().any(|part| part == "Root"), "{command:?}");
        assert!(
            command.iter().any(|part| part.ends_with("driver.cer")),
            "{command:?}"
        );
    }

    /// The hardware id is what the `.inf` names, what Windows shows under the device instance, and what the forced
    /// install is asked to replace — a typo here binds nothing and reports "no such device".
    #[test]
    fn the_hardware_id_is_the_one_the_package_and_the_device_name() {
        assert_eq!(hardware_id(), r"USB\VID_17CC&PID_1720&MI_04");
    }

    /// The offsets below were measured with a C compiler on the declarations copied from `libwdi.h` 1.5.1
    /// (`offsetof`, x86_64). A structure the library writes and this side reads with a missing field would read
    /// somebody else's memory, so the transcription is checked rather than trusted.
    #[test]
    #[cfg(target_pointer_width = "64")]
    fn the_c_structures_have_the_layout_the_header_declares() {
        use std::mem::{offset_of, size_of};

        assert_eq!(size_of::<ffi::DeviceInfo>(), 80);
        assert_eq!(offset_of!(ffi::DeviceInfo, next), 0);
        assert_eq!(offset_of!(ffi::DeviceInfo, vid), 8);
        assert_eq!(offset_of!(ffi::DeviceInfo, pid), 10);
        assert_eq!(offset_of!(ffi::DeviceInfo, is_composite), 12);
        assert_eq!(offset_of!(ffi::DeviceInfo, mi), 16);
        assert_eq!(offset_of!(ffi::DeviceInfo, desc), 24);
        assert_eq!(offset_of!(ffi::DeviceInfo, driver), 32);
        assert_eq!(offset_of!(ffi::DeviceInfo, device_id), 40);
        assert_eq!(offset_of!(ffi::DeviceInfo, hardware_id), 48);
        assert_eq!(offset_of!(ffi::DeviceInfo, compatible_id), 56);
        assert_eq!(offset_of!(ffi::DeviceInfo, upper_filter), 64);
        assert_eq!(offset_of!(ffi::DeviceInfo, driver_version), 72);

        assert_eq!(size_of::<ffi::OptionsCreateList>(), 12);
        assert_eq!(offset_of!(ffi::OptionsCreateList, list_all), 0);
        assert_eq!(offset_of!(ffi::OptionsCreateList, list_hubs), 4);
        assert_eq!(offset_of!(ffi::OptionsCreateList, trim_whitespaces), 8);

        assert_eq!(size_of::<ffi::OptionsPrepare>(), 48);
        assert_eq!(offset_of!(ffi::OptionsPrepare, driver_type), 0);
        assert_eq!(offset_of!(ffi::OptionsPrepare, vendor_name), 8);
        assert_eq!(offset_of!(ffi::OptionsPrepare, device_guid), 16);
        assert_eq!(offset_of!(ffi::OptionsPrepare, disable_cat), 24);
        assert_eq!(offset_of!(ffi::OptionsPrepare, disable_signing), 28);
        assert_eq!(offset_of!(ffi::OptionsPrepare, cert_subject), 32);
        assert_eq!(offset_of!(ffi::OptionsPrepare, use_wcid_driver), 40);
        assert_eq!(offset_of!(ffi::OptionsPrepare, external_inf), 44);
    }

    /// The elevated copy has to be the same command with the marker that stops it from asking for a prompt of its own,
    /// and the report path has to survive the quotes Windows will hand back.
    #[test]
    fn the_elevated_copy_is_the_same_command_with_a_marker_and_a_quoted_report() {
        let report = Path::new("C:\\Users\\uncle\\Temp\\hauler-dj-screens-driver\\report.txt");
        let parameters = elevated_parameters(report);
        assert!(parameters.contains("--screens-driver"), "{parameters}");
        assert!(parameters.contains("--elevated"), "{parameters}");
        assert!(
            parameters.contains(&format!("--report \"{}\"", report.display())),
            "{parameters}"
        );
        // Nothing that would bind the install path: the elevated copy resolves the package itself.
        assert!(!parameters.contains("driver/"), "{parameters}");
    }

    /// The listing has to ask for everything: a null options pointer returns driverless devices only, and the interface
    /// this command exists for is by definition not driverless.
    #[test]
    fn the_listing_asks_for_devices_that_already_have_a_driver() {
        let options = ffi::OptionsCreateList::everything();
        assert_eq!(options.list_all, 1);
        assert_eq!(options.list_hubs, 1);
        assert_eq!(options.trim_whitespaces, 1);
        assert_eq!(ffi::OptionsPrepare::winusb().driver_type, ffi::WDI_WINUSB);
        assert_eq!(ffi::OptionsPrepare::winusb().disable_signing, 0);
    }
}

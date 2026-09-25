//! Rust port of nomi-san/parsec-vdd's `core/parsec-vdd.h` (MIT), the
//! reverse-engineered wrapper around Parsec's Indirect Display Driver (IddCx).
//! Source: <https://github.com/nomi-san/parsec-vdd/blob/master/core/parsec-vdd.h>
//!
//! The driver binary itself (mm.inf/mm.dll/mm.cat — proprietary, built and
//! signed by Parsec) is **not** vendored here and this module does not
//! download or install it. See `docs/vdd-setup.md` for the manual
//! install/uninstall commands.
//!
//! One correction versus the original spec, made after reading the upstream
//! header and docs rather than assuming: the heartbeat must run well under
//! 1 second, not "every 1 second" — the driver's own watchdog unplugs every
//! virtual display if pings stop for about a second, so `HEARTBEAT_INTERVAL`
//! below pings every 50ms instead.
//!
//! The registry step from the spec is real, just not in the core header:
//! the driver reads up to 5 custom-resolution presets from
//! `HKLM\SOFTWARE\Parsec\vdd\<slot>` (REG_DWORD `w`/`h`/`hz`) at adapter
//! init — see `write_custom_resolutions` below and
//! docs/PARSEC_VDD_RE.md §8 / README.md "Custom resolutions" upstream.
//!
//! Verified end-to-end against the real driver (v0.45.0.0) on 2026-09-22:
//! registry write, device open, add-display, heartbeat, and version query
//! all round-trip correctly when run elevated. Getting there surfaced two
//! real bugs worth knowing about if this file needs touching again:
//! - `open_device_handle` must request `GENERIC_READ | GENERIC_WRITE` (the
//!   plain generic rights, object-manager-mapped per device type) on
//!   `CreateFileW`, not `FILE_GENERIC_READ | FILE_GENERIC_WRITE` (the
//!   pre-expanded file-specific rights). The driver's create handler
//!   rejects the latter with `ERROR_INVALID_PARAMETER` — upstream's C code
//!   uses the plain generic form for exactly this reason, and this port
//!   originally deviated from it without cause. Match upstream's access
//!   rights choices literally, not "equivalent-looking" substitutes.
//! - Comparing a `windows::core::Error`'s `.code()` (an `HRESULT`, e.g.
//!   `0x80070103`) against a `WIN32_ERROR`'s bare `.0` (e.g. `0x103`) never
//!   matches even when they represent the same underlying error — convert
//!   one side with `.to_hresult()` first. Getting this wrong in
//!   `open_device_handle`'s "is this just end-of-enumeration" check
//!   silently replaced a real `CreateFileW` error with the benign
//!   `ERROR_NO_MORE_ITEMS` signal, which is what made this bug take several
//!   rounds to actually root-cause.
//!
//! Incident note (2026-09-23) — the `--remove-index` CLI flag in `main.rs`
//! exists because of this: a background test process got force-killed
//! (`taskkill /F`, needed because it wouldn't stop any other way from a
//! different console session), which skips `VddSession::Drop` and leaks its
//! display on the shared adapter forever — nothing else ever removes it.
//! Cleaning that up meant guessing which of two ambiguous indices was the
//! leak versus the display an already-running Parsec session actually
//! depended on, since the driver has no "query what's in slot N" IOCTL.
//! That guess was wrong: `--remove-index 0` turned out to remove the live
//! session's display, not the orphan. The actual, hard-won lesson: **don't
//! force-kill a process that owns a `VddSession` on this shared adapter.**
//! Let it shut down on its own (`--stream-test` runs for a fixed duration
//! and exits cleanly, precisely to avoid ever needing this), or if you must
//! kill one, verify which index it held *before* doing so — after the fact,
//! there is no reliable way to tell displays apart by index alone; see the
//! matching incident note in `capture.rs` for what this ultimately fed
//! into (a persistently poisoned `ACCESS_LOST` state, still unresolved).

use std::ffi::c_void;
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use windows::core::{GUID, PCWSTR};
use windows::Win32::Devices::Display::{SetDisplayConfig, SDC_APPLY, SDC_TOPOLOGY_EXTEND};
use windows::Win32::Devices::DeviceAndDriverInstallation::{
    SetupDiDestroyDeviceInfoList, SetupDiEnumDeviceInterfaces, SetupDiGetClassDevsW,
    SetupDiGetDeviceInterfaceDetailW, DIGCF_DEVICEINTERFACE, DIGCF_PRESENT,
    SP_DEVICE_INTERFACE_DATA, SP_DEVICE_INTERFACE_DETAIL_DATA_W,
};
use windows::Win32::Foundation::{
    CloseHandle, ERROR_NO_MORE_ITEMS, GENERIC_READ, GENERIC_WRITE, HANDLE,
};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_FLAG_NO_BUFFERING, FILE_FLAG_OVERLAPPED,
    FILE_FLAG_WRITE_THROUGH, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegSetValueExW, HKEY_LOCAL_MACHINE, KEY_READ, KEY_WRITE,
    REG_DWORD, REG_OPTION_NON_VOLATILE,
};
use windows::Win32::System::Threading::CreateEventW;
use windows::Win32::System::IO::{DeviceIoControl, GetOverlappedResultEx, OVERLAPPED};

/// Adapter/interface GUID used to obtain the device handle.
/// `{00b41627-04c4-429e-a26e-0265cf50c8fa}`
pub const VDD_ADAPTER_GUID: GUID = GUID::from_values(
    0x00b4_1627,
    0x04c4,
    0x429e,
    [0xa2, 0x6e, 0x02, 0x65, 0xcf, 0x50, 0xc8, 0xfa],
);

/// Friendly device string Windows shows for this display in the [Advanced
/// display settings] tab — NOT what `EnumDisplayDevicesW`/`capture.rs` need
/// (that's `VDD_ADAPTER_NAME` below, a different level of the device tree).
#[allow(dead_code)]
pub const VDD_DISPLAY_NAME: &str = "ParsecVDA";

/// Friendly *adapter* name — what `EnumDisplayDevicesW(None, index, ...)`
/// returns for this device (confirmed via `Get-PnpDevice` while debugging
/// module 2: `FriendlyName = "Parsec Virtual Display Adapter"`). This is
/// the one `capture.rs` matches against to find the right DXGI output;
/// `VDD_DISPLAY_NAME` looks similar but is queried a level lower (the
/// monitor attached to the adapter, which this virtual display doesn't
/// necessarily have) and won't match there.
pub const VDD_ADAPTER_NAME: &str = "Parsec Virtual Display Adapter";

// Reserved for a future `query_device_status` (checks install/driver state
// via these before `open_device_handle` is even attempted, so a missing
// driver reports as "not installed" instead of a bare CreateFile failure)
// and for multi-display support — neither is wired up yet, hence the
// `allow`. See upstream `QueryDeviceStatus` in parsec-vdd.h for the port
// this would follow.
#[allow(dead_code)]
/// Standard "Display" device class GUID — used to query install/driver
/// status, not to open the device itself.
/// `{4d36e968-e325-11ce-bfc1-08002be10318}`
pub const VDD_CLASS_GUID: GUID = GUID::from_values(
    0x4d36_e968,
    0xe325,
    0x11ce,
    [0xbf, 0xc1, 0x08, 0x00, 0x2b, 0xe1, 0x03, 0x18],
);

#[allow(dead_code)]
pub const VDD_HARDWARE_ID: &str = r"Root\Parsec\VDA";
#[allow(dead_code)]
pub const VDD_MAX_DISPLAYS: u32 = 8;

/// The driver's own watchdog removes every virtual display if it doesn't
/// see a ping for ~1s (per upstream docs) — this must stay well under that.
const HEARTBEAT_INTERVAL: Duration = Duration::from_millis(50);

const VDD_IOCTL_ADD: u32 = 0x0022_e004;
const VDD_IOCTL_REMOVE: u32 = 0x0022_a008;
const VDD_IOCTL_UPDATE: u32 = 0x0022_a00c;
const VDD_IOCTL_VERSION: u32 = 0x0022_e010;

/// Number of custom-resolution slots the driver parses — fixed in the
/// driver-side parsing, not something the registry format can exceed.
pub const VDD_MAX_CUSTOM_RESOLUTIONS: usize = 5;

/// Writes up to [`VDD_MAX_CUSTOM_RESOLUTIONS`] custom `(width, height, hz)`
/// presets to `HKLM\SOFTWARE\Parsec\vdd\<slot>` (slots 0..5). The driver
/// only reads these at adapter init, so this must run *before*
/// `VddSession::start` opens the device — writing after `VddAddDisplay` has
/// no effect on an already-plugged display.
pub fn write_custom_resolutions(resolutions: &[(u32, u32, u32)]) -> io::Result<()> {
    for (slot, &(width, height, hz)) in resolutions.iter().enumerate().take(VDD_MAX_CUSTOM_RESOLUTIONS) {
        write_resolution_slot(slot as u32, width, height, hz)?;
    }
    Ok(())
}

fn write_resolution_slot(slot: u32, width: u32, height: u32, hz: u32) -> io::Result<()> {
    let subkey_path: Vec<u16> = format!("SOFTWARE\\Parsec\\vdd\\{slot}")
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();

    unsafe {
        let mut hkey = Default::default();
        RegCreateKeyExW(
            HKEY_LOCAL_MACHINE,
            PCWSTR(subkey_path.as_ptr()),
            None,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_READ | KEY_WRITE,
            None,
            &mut hkey,
            None,
        )
        .ok()
        .map_err(|e| io::Error::from_raw_os_error(e.code().0))?;

        let write_result = set_dword(hkey, "w", width)
            .and_then(|_| set_dword(hkey, "h", height))
            .and_then(|_| set_dword(hkey, "hz", hz));

        let _ = RegCloseKey(hkey);
        write_result
    }
}

unsafe fn set_dword(hkey: windows::Win32::System::Registry::HKEY, name: &str, value: u32) -> io::Result<()> {
    let name_w: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
    let bytes = value.to_le_bytes();
    RegSetValueExW(hkey, PCWSTR(name_w.as_ptr()), Some(0), REG_DWORD, Some(&bytes))
        .ok()
        .map_err(|e| io::Error::from_raw_os_error(e.code().0))
}

/// Opens a handle to the VDD device interface. Mirrors `OpenDeviceHandle` in
/// the upstream header: there is no fixed symbolic link name, the device
/// path is resolved at runtime via SetupAPI device-interface enumeration.
///
/// Unlike upstream (which just returns NULL on any failure), this preserves
/// the real Win32 error from whichever step actually failed and how many
/// interfaces were even found — "no interfaces registered for this GUID" vs
/// "found one, CreateFile was denied" are different problems (GUID/driver
/// mismatch vs. needs elevation) that upstream's bare HANDLE return can't
/// tell apart, and neither could an earlier version of this port.
pub fn open_device_handle(interface_guid: &GUID) -> io::Result<HANDLE> {
    unsafe {
        let dev_info = SetupDiGetClassDevsW(
            Some(interface_guid),
            PCWSTR::null(),
            None,
            DIGCF_PRESENT | DIGCF_DEVICEINTERFACE,
        )
        .map_err(|e| io::Error::from_raw_os_error(e.code().0))?;

        let mut index = 0u32;
        let mut interfaces_seen = 0u32;
        let mut last_error: Option<io::Error> = None;

        let result = loop {
            let mut iface_data = SP_DEVICE_INTERFACE_DATA {
                cbSize: std::mem::size_of::<SP_DEVICE_INTERFACE_DATA>() as u32,
                ..Default::default()
            };

            if let Err(e) =
                SetupDiEnumDeviceInterfaces(dev_info, None, interface_guid, index, &mut iface_data)
            {
                // ERROR_NO_MORE_ITEMS is the expected "done enumerating"
                // signal, not a real failure — anything else is. Comparing
                // HRESULTs on both sides here matters: `e.code()` is an
                // HRESULT (e.g. 0x80070103), while `ERROR_NO_MORE_ITEMS.0`
                // is the bare Win32 code (0x103) — comparing those directly
                // never matches, which previously let this branch clobber a
                // real CreateFileW error with this benign one.
                if e.code() != ERROR_NO_MORE_ITEMS.to_hresult() {
                    last_error = Some(io::Error::from_raw_os_error(e.code().0));
                }
                break Err(());
            }
            interfaces_seen += 1;

            let mut required_size = 0u32;
            let _ = SetupDiGetDeviceInterfaceDetailW(
                dev_info,
                &iface_data,
                None,
                0,
                Some(&mut required_size),
                None,
            );

            if required_size > 0 {
                let mut buffer = vec![0u8; required_size as usize];
                let detail = buffer.as_mut_ptr() as *mut SP_DEVICE_INTERFACE_DETAIL_DATA_W;
                (*detail).cbSize = std::mem::size_of::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>() as u32;

                match SetupDiGetDeviceInterfaceDetailW(
                    dev_info,
                    &iface_data,
                    Some(detail),
                    required_size,
                    None,
                    None,
                ) {
                    Ok(()) => {
                        let device_path = PCWSTR((*detail).DevicePath.as_ptr());
                        log::debug!("VDD: trying device interface at {}", device_path.display());
                        match CreateFileW(
                            device_path,
                            (GENERIC_READ | GENERIC_WRITE).0,
                            FILE_SHARE_READ | FILE_SHARE_WRITE,
                            None,
                            OPEN_EXISTING,
                            FILE_ATTRIBUTE_NORMAL
                                | FILE_FLAG_NO_BUFFERING
                                | FILE_FLAG_OVERLAPPED
                                | FILE_FLAG_WRITE_THROUGH,
                            None,
                        ) {
                            Ok(handle) if !handle.is_invalid() => {
                                log::debug!("VDD: CreateFileW succeeded, handle is open");
                                break Ok(handle);
                            }
                            Ok(_) => {
                                log::debug!("VDD: CreateFileW returned an invalid handle (no error code)");
                                last_error = Some(io::Error::other("CreateFileW returned an invalid handle"))
                            }
                            Err(e) => {
                                log::debug!("VDD: CreateFileW failed: {e}");
                                last_error = Some(io::Error::from_raw_os_error(e.code().0));
                            }
                        }
                    }
                    Err(e) => last_error = Some(io::Error::from_raw_os_error(e.code().0)),
                }
            }

            index += 1;
        };

        let _ = SetupDiDestroyDeviceInfoList(dev_info);
        log::debug!("VDD: saw {interfaces_seen} device interface(s) for {{{interface_guid:?}}}");

        result.map_err(|_| {
            last_error.unwrap_or_else(|| {
                io::Error::new(
                    io::ErrorKind::NotFound,
                    format!(
                        "no device interfaces registered for {{{interface_guid:?}}} \
                         (saw {interfaces_seen} total) — driver not loaded, or this GUID \
                         doesn't match the installed driver version"
                    ),
                )
            })
        })
    }
}

pub fn close_device_handle(handle: HANDLE) {
    if !handle.is_invalid() {
        unsafe {
            let _ = CloseHandle(handle);
        }
    }
}

/// Generic `DeviceIoControl` wrapper, mirroring `VddIoControl` upstream:
/// fixed 32-byte input buffer, a single DWORD of output, overlapped I/O
/// with a 5s wait for completion.
fn vdd_io_control(handle: HANDLE, code: u32, data: &[u8]) -> io::Result<u32> {
    unsafe {
        let mut in_buffer = [0u8; 32];
        let copy_len = data.len().min(in_buffer.len());
        in_buffer[..copy_len].copy_from_slice(&data[..copy_len]);

        let event = CreateEventW(None, true, false, None)?;
        let mut overlapped = OVERLAPPED {
            hEvent: event,
            ..Default::default()
        };

        let mut out_buffer: u32 = 0;
        let mut bytes_returned: u32 = 0;

        // Upstream ignores DeviceIoControl's own BOOL return and instead
        // waits on the overlapped result below; matched here for fidelity.
        // An immediate `ERROR_IO_PENDING` is the expected/normal outcome
        // for a queued overlapped op, not a failure — hence `trace!`, not
        // `warn!`: this fires on every heartbeat tick (every 50ms) once
        // things are working, and logging it at `debug` drowns out
        // everything else. Use `RUST_LOG=trace` if you need to see it again.
        let immediate = DeviceIoControl(
            handle,
            code,
            Some(in_buffer.as_ptr() as *const c_void),
            in_buffer.len() as u32,
            Some(&mut out_buffer as *mut u32 as *mut c_void),
            std::mem::size_of::<u32>() as u32,
            None,
            Some(&mut overlapped),
        );
        log::trace!("VDD: DeviceIoControl(code=0x{code:08x}) immediate result: {immediate:?}");

        let wait_result = GetOverlappedResultEx(handle, &overlapped, &mut bytes_returned, 5000, false);
        let _ = CloseHandle(event);
        log::trace!(
            "VDD: GetOverlappedResultEx(code=0x{code:08x}) -> {wait_result:?}, bytes_returned={bytes_returned}, out_buffer={out_buffer}"
        );
        wait_result?;

        Ok(out_buffer)
    }
}

pub fn vdd_version(handle: HANDLE) -> io::Result<u32> {
    vdd_io_control(handle, VDD_IOCTL_VERSION, &[])
}

/// Pings the driver to keep every currently-added virtual display alive.
/// Must be called on a fixed cadence well under 1s — see `HEARTBEAT_INTERVAL`.
pub fn vdd_update(handle: HANDLE) {
    if let Err(e) = vdd_io_control(handle, VDD_IOCTL_UPDATE, &[]) {
        // Not silently discarded: if pings start failing, the driver tears
        // the display down in ~1s, which should be visible in the log
        // rather than discovered only when the display disappears.
        log::warn!("VDD: heartbeat ping failed: {e}");
    }
}

/// Adds/plugs a virtual display. Returns its index.
pub fn vdd_add_display(handle: HANDLE) -> io::Result<u32> {
    let index = vdd_io_control(handle, VDD_IOCTL_ADD, &[])?;
    vdd_update(handle);
    Ok(index)
}

/// Removes/unplugs a virtual display by index.
pub fn vdd_remove_display(handle: HANDLE, index: u16) {
    let _ = vdd_io_control(handle, VDD_IOCTL_REMOVE, &index.to_be_bytes());
    vdd_update(handle);
}

/// Forces Windows to extend the desktop across every currently-available
/// display. `VddAddDisplay` makes the driver report a new output, but that
/// alone doesn't attach it to the composited desktop — confirmed via
/// `[System.Windows.Forms.Screen]::AllScreens` while debugging module 3:
/// a freshly-added display was absent from it, and DXGI Desktop Duplication
/// failed with `DXGI_ERROR_ACCESS_LOST` persistently (not just once) as a
/// direct result. Passing no explicit path/mode arrays tells Windows to
/// compute the extended layout itself, which only *attaches* previously
/// inactive displays — it doesn't reposition or otherwise disturb ones
/// that are already active.
pub fn extend_desktop_onto_all_displays() -> io::Result<()> {
    let result = unsafe { SetDisplayConfig(None, None, SDC_APPLY | SDC_TOPOLOGY_EXTEND) };
    if result != 0 {
        return Err(io::Error::from_raw_os_error(result));
    }
    Ok(())
}

/// Owns the lifecycle of one virtual display plus its heartbeat thread:
/// adds the display on construction, pings it in the background, and
/// removes it on drop. This is the "clean up if the client disconnects"
/// behavior from the spec — whatever tracks the client's connection
/// (module 3, not yet implemented) should hold this session and let it
/// drop when that connection ends, rather than wiring a separate callback.
pub struct VddSession {
    handle: HANDLE,
    display_index: u32,
    stop_heartbeat: Arc<AtomicBool>,
    heartbeat_thread: Option<std::thread::JoinHandle<()>>,
}

// SAFETY: `handle` is a plain kernel handle value (Copy, no interior
// mutability); the driver itself serializes concurrent IOCTLs the way any
// WDF device does, so sharing it between the heartbeat thread and the
// owning thread is sound.
unsafe impl Send for VddSession {}

/// `HANDLE` wraps a raw pointer so it isn't `Send` by default; see the
/// safety note on `impl Send for VddSession` above — the same reasoning
/// applies here.
struct SendHandle(HANDLE);
unsafe impl Send for SendHandle {}

impl VddSession {
    pub fn start(interface_guid: &GUID) -> io::Result<Self> {
        let handle = open_device_handle(interface_guid)?;

        let display_index = match vdd_add_display(handle) {
            Ok(idx) => idx,
            Err(e) => {
                close_device_handle(handle);
                return Err(e);
            }
        };
        log::info!("VDD: added virtual display index {display_index}");

        // The driver reporting a new output doesn't mean Windows has
        // attached it to the composited desktop — see
        // `extend_desktop_onto_all_displays`'s doc comment. Without this,
        // the display exists but nothing can actually capture from it.
        if let Err(e) = extend_desktop_onto_all_displays() {
            log::warn!(
                "VDD: couldn't force-extend the desktop onto the new display ({e}) — it may not \
                 be capturable until something else attaches it (e.g. opening Display Settings)"
            );
        }

        let stop_heartbeat = Arc::new(AtomicBool::new(false));
        let heartbeat_thread = {
            let stop = stop_heartbeat.clone();
            // HANDLE wraps a raw pointer, so it isn't Send by default; see
            // the `unsafe impl Send for VddSession` note below for why
            // sharing it with the heartbeat thread is sound here.
            let handle = SendHandle(handle);
            std::thread::spawn(move || {
                // Capture the whole `SendHandle`, not just its `.0` field —
                // edition 2021's disjoint closure capture would otherwise
                // capture the bare (non-Send) `HANDLE` field directly and
                // defeat the wrapper above.
                let handle = handle;
                while !stop.load(Ordering::Relaxed) {
                    vdd_update(handle.0);
                    std::thread::sleep(HEARTBEAT_INTERVAL);
                }
            })
        };

        Ok(Self {
            handle,
            display_index,
            stop_heartbeat,
            heartbeat_thread: Some(heartbeat_thread),
        })
    }

    pub fn display_index(&self) -> u32 {
        self.display_index
    }

    /// Queries the driver's minor version over the same handle — useful as
    /// a startup sanity check that the IOCTL round-trip actually works.
    pub fn driver_version(&self) -> io::Result<u32> {
        vdd_version(self.handle)
    }
}

impl Drop for VddSession {
    fn drop(&mut self) {
        self.stop_heartbeat.store(true, Ordering::Relaxed);
        if let Some(t) = self.heartbeat_thread.take() {
            let _ = t.join();
        }

        log::info!("VDD: removing virtual display index {}", self.display_index);
        vdd_remove_display(self.handle, self.display_index as u16);
        close_device_handle(self.handle);
    }
}

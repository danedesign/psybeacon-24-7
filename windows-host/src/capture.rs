//! DXGI Desktop Duplication capture, scoped to the specific PsychBeacon
//! virtual display this process just added — not just any display on the
//! shared Parsec VDD adapter. See `for_new_output`'s doc comment for why
//! that distinction is load-bearing, not cosmetic.
//!
//! Incident note (2026-09-22): this dev machine runs headless, and the
//! *existing* Parsec app was already using the same shared
//! "Parsec Virtual Display Adapter" (one adapter instance, `ROOT\DISPLAY\0001`,
//! can host multiple virtual displays) to serve the live remote session this
//! code was being developed through. An earlier version of this module
//! matched outputs by adapter friendly name alone, which can't distinguish
//! "the display this process just added" from "whatever was already there"
//! — it worked only by enumeration-order luck, and in the wrong order would
//! have opened desktop duplication directly on someone's live session
//! instead of our own test display. `for_new_output` fixes this properly:
//! snapshot outputs before adding a display, diff after, and only ever open
//! the one that's actually new.
//!
//! Status as of 2026-09-25 — capture is currently broken, parked, not
//! fixed: `AcquireNextFrame` fails immediately and persistently with
//! `DXGI_ERROR_ACCESS_LOST` ("the keyed mutex was abandoned"), surviving
//! `recreate_duplication`'s recovery (recreate + re-extend), elevation
//! (tested both ways, identical failure), and 2.5s of backoff retries.
//! `DISPLAY_DEVICE_ACTIVE` reads `true` throughout, so — unlike the
//! `SetDisplayConfig` issue `recreate_duplication`'s doc comment describes
//! — this is not the display failing to attach.
//!
//! Leading theory: a genuinely abandoned `IDXGIKeyedMutex`. Desktop
//! Duplication's shared surface is synchronized via a keyed mutex between
//! the compositor and whatever's consuming it; "abandoned" is the specific,
//! literal meaning of that error text, not just generic HRESULT-to-string
//! filler — it means whatever held the lock terminated without releasing
//! it, which poisons the shared resource for every subsequent consumer
//! regardless of which process asks or with what privilege. This lines up
//! with the timeline: this started failing only after a debugging session
//! that force-killed several `psybeacon-host.exe` processes while they
//! likely held active duplication state (chasing down orphaned displays —
//! see `vdd.rs`'s incident notes for that whole story). A force-kill runs
//! no cleanup, so it's a plausible way to leave exactly this behind, on the
//! *shared* adapter a live session also depends on.
//!
//! Fixing a poisoned compositor-level lock needs something that resets it
//! — restarting `dwm.exe`, disabling/re-enabling the adapter, or a reboot
//! are the candidates — and all three would visibly disrupt whatever's
//! currently rendering on this machine, including a live remote session.
//! Deliberately not attempted without the machine's owner explicitly
//! choosing to accept that disruption at a time of their choosing — see the
//! conversation this was debugged in, where acting on a similar theory
//! about shared state without checking first went wrong once already.
//! Whoever picks this back up: confirm this theory (or find a better one)
//! before reaching for a fix that touches shared state again.
//!
//! Not zero-copy yet: `read_back_frame` does a GPU→CPU copy
//! (`CopyResource` into a staging texture, then `Map`) so the bytes can be
//! written to a subprocess's stdin pipe in `main.rs`. True zero-copy needs
//! either direct NVENC SDK integration (hand the D3D11 texture straight to
//! NVENC without a CPU round-trip) or in-process libavcodec with a D3D11VA
//! `AVHWFramesContext` wrapping the same texture — both are real follow-up
//! work, not implemented here. See CLAUDE.md module 3 notes.

use std::io;
use std::time::Duration;

use windows::core::Interface;
use windows::core::PCWSTR;
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_UNKNOWN;
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D,
    D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAP_READ,
    D3D11_MAPPED_SUBRESOURCE, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, IDXGIAdapter, IDXGIFactory1, IDXGIOutput, IDXGIOutput1,
    IDXGIOutputDuplication, IDXGIResource, DXGI_ERROR_ACCESS_LOST, DXGI_ERROR_WAIT_TIMEOUT,
    DXGI_OUTDUPL_FRAME_INFO,
};
use windows::Win32::Graphics::Gdi::{EnumDisplayDevicesW, DISPLAY_DEVICEW, DISPLAY_DEVICE_ACTIVE};

/// A single captured frame, already read back into system memory as tightly
/// packed BGRA8 (row padding from `RowPitch` stripped during readback).
#[allow(dead_code)] // width/height: unused once a frame only ever gets piped
                     // straight into ffmpeg (which is told the dimensions
                     // separately) rather than saved standalone — kept on
                     // the struct since save_bmp below needs them, and
                     // dropping them would just mean re-adding them the
                     // next time this needs debugging visually.
pub struct CapturedFrame {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

impl CapturedFrame {
    /// Writes this frame as an uncompressed 32bpp BMP — a quick,
    /// dependency-free way to visually confirm real pixels were captured,
    /// not a format anything downstream should consume. Not currently
    /// called from `main.rs`'s paths; keep it around for the next time
    /// capture output needs eyeballing rather than re-deriving it.
    #[allow(dead_code)]
    pub fn save_bmp(&self, path: &std::path::Path) -> io::Result<()> {
        let pixel_bytes = self.data.len() as u32;
        let file_size = 14 + 40 + pixel_bytes;

        let mut out = Vec::with_capacity(file_size as usize);
        out.extend_from_slice(b"BM");
        out.extend_from_slice(&file_size.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes()); // reserved
        out.extend_from_slice(&(14u32 + 40).to_le_bytes()); // pixel data offset

        out.extend_from_slice(&40u32.to_le_bytes()); // BITMAPINFOHEADER size
        out.extend_from_slice(&(self.width as i32).to_le_bytes());
        out.extend_from_slice(&(-(self.height as i64) as i32).to_le_bytes()); // negative = top-down
        out.extend_from_slice(&1u16.to_le_bytes()); // planes
        out.extend_from_slice(&32u16.to_le_bytes()); // bpp
        out.extend_from_slice(&0u32.to_le_bytes()); // BI_RGB, uncompressed
        out.extend_from_slice(&pixel_bytes.to_le_bytes());
        out.extend_from_slice(&0i32.to_le_bytes());
        out.extend_from_slice(&0i32.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());

        out.extend_from_slice(&self.data);
        std::fs::write(path, out)
    }
}

pub struct DesktopDuplicator {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    output1: IDXGIOutput1,
    duplication: IDXGIOutputDuplication,
    /// `\\.\DISPLAYN` — kept for `is_active`'s diagnostic check during
    /// ACCESS_LOST recovery, not used for opening (that's done by COM
    /// object, not by re-looking-up this name).
    device_name: String,
    width: u32,
    height: u32,
}

impl DesktopDuplicator {
    /// Opens desktop duplication on whichever output on the adapter
    /// matching `adapter_name_substring` (e.g. "Parsec Virtual Display
    /// Adapter") is present now but wasn't in `before` — see
    /// [`snapshot_matching_outputs`]. This, not name-matching alone, is how
    /// "our" display gets identified: the adapter can be shared with other
    /// displays that were already there (a pre-existing Parsec session, for
    /// instance), and matching by friendly name alone can't tell those
    /// apart from the one this process just added — see the module-level
    /// incident note for why that distinction matters here specifically.
    pub fn for_new_output(
        adapter_name_substring: &str,
        before: &std::collections::HashSet<String>,
    ) -> io::Result<Self> {
        let after = enumerate_matching_outputs(adapter_name_substring)?;
        let mut new_ones: Vec<_> = after
            .into_iter()
            .filter(|(name, _, _)| !before.contains(name))
            .collect();

        match new_ones.len() {
            0 => Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!(
                    "no new output appeared on the \"{adapter_name_substring}\" adapter since \
                     the snapshot — nothing to safely open"
                ),
            )),
            1 => {
                let (name, adapter, output) = new_ones.remove(0);
                log::info!("Capture: targeting newly-added output {name}");
                unsafe { Self::open(&adapter, &output, name) }
            }
            n => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "{n} new outputs appeared since the snapshot, not 1 — refusing to guess \
                     which one is ours"
                ),
            )),
        }
    }

    /// Snapshot of `\\.\DISPLAYN` device names currently on the adapter
    /// matching `adapter_name_substring`, for [`for_new_output`]'s
    /// before/after diff. Call this *before* `VddSession::start` adds a
    /// display, not after.
    pub fn snapshot_matching_outputs(
        adapter_name_substring: &str,
    ) -> io::Result<std::collections::HashSet<String>> {
        Ok(enumerate_matching_outputs(adapter_name_substring)?
            .into_iter()
            .map(|(name, _, _)| name)
            .collect())
    }

    unsafe fn open(adapter: &IDXGIAdapter, output: &IDXGIOutput, device_name: String) -> io::Result<Self> {
        let mut device: Option<ID3D11Device> = None;
        let mut context: Option<ID3D11DeviceContext> = None;

        D3D11CreateDevice(
            adapter,
            D3D_DRIVER_TYPE_UNKNOWN, // must be UNKNOWN when an explicit adapter is passed
            windows::Win32::Foundation::HMODULE::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            None,
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            Some(&mut context),
        )
        .map_err(|e| io::Error::from_raw_os_error(e.code().0))?;

        let device = device.ok_or_else(|| io::Error::other("D3D11CreateDevice gave no device"))?;
        let context =
            context.ok_or_else(|| io::Error::other("D3D11CreateDevice gave no context"))?;

        let output1: IDXGIOutput1 = output
            .cast()
            .map_err(|e| io::Error::from_raw_os_error(e.code().0))?;
        let duplication = output1
            .DuplicateOutput(&device)
            .map_err(|e| io::Error::from_raw_os_error(e.code().0))?;

        let desc = output
            .GetDesc()
            .map_err(|e| io::Error::from_raw_os_error(e.code().0))?;
        let width = (desc.DesktopCoordinates.right - desc.DesktopCoordinates.left) as u32;
        let height = (desc.DesktopCoordinates.bottom - desc.DesktopCoordinates.top) as u32;
        log::info!("Capture: duplicating output at {width}x{height}");

        Ok(Self {
            device,
            context,
            output1,
            duplication,
            device_name,
            width,
            height,
        })
    }

    /// Re-asserts extended-desktop topology, then drops the current
    /// duplication interface and opens a fresh one on the same output.
    /// Required after `DXGI_ERROR_ACCESS_LOST`.
    ///
    /// Two distinct failure modes both surface as this same error, which is
    /// why this does both things rather than just recreating duplication:
    /// a genuinely-attached display whose duplication object died (a
    /// transient compositor hiccup, e.g. from a lock-screen/secure-desktop
    /// transition) just needs a fresh `DuplicateOutput` — but a display
    /// that was never actually attached to the composited desktop in the
    /// first place (confirmed via `[System.Windows.Forms.Screen]::AllScreens`
    /// while debugging: it was missing entirely) fails identically, and no
    /// amount of recreating duplication helps there since the real problem
    /// is upstream of DXGI. `VddSession::start` already calls
    /// `vdd::extend_desktop_onto_all_displays` once, but that single call
    /// isn't reliably sufficient — observed failing silently (no error
    /// returned, display still absent from `AllScreens`) on a second,
    /// otherwise-identical run two days after the first fix was verified,
    /// same machine, same code. Retrying the extend call here as well, on
    /// every recovery attempt, is what actually closes that gap.
    fn recreate_duplication(&mut self) -> io::Result<()> {
        if let Err(e) = crate::vdd::extend_desktop_onto_all_displays() {
            log::warn!("Capture: re-extend attempt during recovery failed: {e}");
        }
        log::warn!(
            "Capture: {} DISPLAY_DEVICE_ACTIVE = {}",
            self.device_name,
            is_display_active(&self.device_name)
        );
        self.duplication = unsafe { self.output1.DuplicateOutput(&self.device) }
            .map_err(|e| io::Error::from_raw_os_error(e.code().0))?;
        Ok(())
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    /// Retry delays after an ACCESS_LOST recreate — increasing, not
    /// immediate: a fresh duplication object failing the *instant* it's
    /// created (as opposed to failing later, mid-stream) points at the
    /// desktop compositor not having started rendering to a just-added
    /// display yet, not at anything a same-instant retry can fix. Total
    /// budget ~2.5s across 3 attempts before giving up for real.
    const ACCESS_LOST_RETRY_DELAYS: [Duration; 3] = [
        Duration::from_millis(250),
        Duration::from_millis(750),
        Duration::from_millis(1500),
    ];

    /// Blocks up to `timeout` for the next changed frame. `Ok(None)` means
    /// the wait elapsed with nothing new (static desktop) — normal, not an
    /// error; the caller should just call this again.
    pub fn capture_next_frame(&mut self, timeout: Duration) -> io::Result<Option<CapturedFrame>> {
        self.capture_next_frame_inner(timeout, &Self::ACCESS_LOST_RETRY_DELAYS)
    }

    fn capture_next_frame_inner(
        &mut self,
        timeout: Duration,
        remaining_retry_delays: &[Duration],
    ) -> io::Result<Option<CapturedFrame>> {
        unsafe {
            let mut frame_info = DXGI_OUTDUPL_FRAME_INFO::default();
            let mut resource: Option<IDXGIResource> = None;

            let acquire_result = self.duplication.AcquireNextFrame(
                timeout.as_millis() as u32,
                &mut frame_info,
                &mut resource,
            );

            let resource = match acquire_result {
                Ok(()) => resource
                    .ok_or_else(|| io::Error::other("AcquireNextFrame gave no resource"))?,
                Err(e) if e.code() == DXGI_ERROR_WAIT_TIMEOUT => return Ok(None),
                Err(e) if e.code() == DXGI_ERROR_ACCESS_LOST => {
                    let Some((&delay, rest)) = remaining_retry_delays.split_first() else {
                        return Err(io::Error::from_raw_os_error(e.code().0));
                    };
                    log::warn!(
                        "Capture: access to the display was lost (topology/mode change, secure \
                         desktop, etc.) — recreating duplication, retrying in {delay:?} \
                         ({} attempt(s) left after this)",
                        rest.len()
                    );
                    self.recreate_duplication()?;
                    std::thread::sleep(delay);
                    return self.capture_next_frame_inner(timeout, rest);
                }
                Err(e) => return Err(io::Error::from_raw_os_error(e.code().0)),
            };

            let result = self.read_back_frame(&resource);
            let _ = self.duplication.ReleaseFrame();
            result.map(Some)
        }
    }

    unsafe fn read_back_frame(&self, resource: &IDXGIResource) -> io::Result<CapturedFrame> {
        let texture: ID3D11Texture2D = resource
            .cast()
            .map_err(|e| io::Error::from_raw_os_error(e.code().0))?;

        let mut desc = D3D11_TEXTURE2D_DESC::default();
        texture.GetDesc(&mut desc);

        let staging_desc = D3D11_TEXTURE2D_DESC {
            Usage: D3D11_USAGE_STAGING,
            BindFlags: 0,
            CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
            MiscFlags: 0,
            ..desc
        };

        let mut staging: Option<ID3D11Texture2D> = None;
        self.device
            .CreateTexture2D(&staging_desc, None, Some(&mut staging))
            .map_err(|e| io::Error::from_raw_os_error(e.code().0))?;
        let staging =
            staging.ok_or_else(|| io::Error::other("CreateTexture2D gave no texture"))?;

        self.context.CopyResource(&staging, &texture);

        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        self.context
            .Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
            .map_err(|e| io::Error::from_raw_os_error(e.code().0))?;

        let width = desc.Width;
        let height = desc.Height;
        let mut data = Vec::with_capacity((width * height * 4) as usize);
        let src = mapped.pData as *const u8;
        for row in 0..height {
            let row_start = src.add((row * mapped.RowPitch) as usize);
            data.extend_from_slice(std::slice::from_raw_parts(row_start, (width * 4) as usize));
        }

        self.context.Unmap(&staging, 0);

        Ok(CapturedFrame {
            width,
            height,
            data,
        })
    }
}

fn decode_wide(buf: &[u16]) -> String {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
}

/// Every currently-enumerated output whose *adapter* friendly name contains
/// `adapter_name_substring`, scanning every DXGI adapter (not just the
/// default one) since the virtual display isn't guaranteed to be on
/// whichever adapter D3D11 would pick by default. Returns the output's
/// `\\.\DISPLAYN` device name alongside the live COM objects so callers can
/// either just collect the names (snapshotting) or open one of them.
fn enumerate_matching_outputs(
    adapter_name_substring: &str,
) -> io::Result<Vec<(String, IDXGIAdapter, IDXGIOutput)>> {
    let needle = adapter_name_substring.to_lowercase();
    let mut matches = Vec::new();

    unsafe {
        let factory: IDXGIFactory1 =
            CreateDXGIFactory1().map_err(|e| io::Error::from_raw_os_error(e.code().0))?;

        let mut adapter_index = 0u32;
        loop {
            let adapter: IDXGIAdapter = match factory.EnumAdapters(adapter_index) {
                Ok(a) => a,
                Err(_) => break,
            };

            let mut output_index = 0u32;
            loop {
                let output: IDXGIOutput = match adapter.EnumOutputs(output_index) {
                    Ok(o) => o,
                    Err(_) => break,
                };

                if let Ok(desc) = output.GetDesc() {
                    let device_name = decode_wide(&desc.DeviceName);
                    let friendly = adapter_friendly_name(&device_name);
                    log::debug!("Capture: output {device_name} = {friendly:?}");
                    if friendly.is_some_and(|f| f.to_lowercase().contains(&needle)) {
                        matches.push((device_name, adapter.clone(), output));
                    }
                }

                output_index += 1;
            }

            adapter_index += 1;
        }
    }

    Ok(matches)
}

/// Whether Windows currently considers this `\\.\DISPLAYN` path part of the
/// active desktop (`DISPLAY_DEVICE_ACTIVE`) — the same thing
/// `[System.Windows.Forms.Screen]::AllScreens` reflects from the .NET side,
/// checked directly here instead of shelling out to PowerShell so recovery
/// logic in `recreate_duplication` can see it inline.
fn is_display_active(device_name: &str) -> bool {
    let mut index = 0u32;
    loop {
        let mut dd = DISPLAY_DEVICEW {
            cb: std::mem::size_of::<DISPLAY_DEVICEW>() as u32,
            ..Default::default()
        };
        let ok = unsafe { EnumDisplayDevicesW(PCWSTR::null(), index, &mut dd, 0) };
        if !ok.as_bool() {
            return false; // exhausted every adapter without a match
        }
        if decode_wide(&dd.DeviceName).eq_ignore_ascii_case(device_name) {
            return (dd.StateFlags & DISPLAY_DEVICE_ACTIVE).0 != 0;
        }
        index += 1;
    }
}

/// Looks up the *adapter*-level friendly name for a `\\.\DISPLAYN` path by
/// enumerating adapters (`lpDevice = NULL`, iterating `iDevNum`) rather than
/// querying that path as `lpDevice` directly — the latter queries the
/// *monitor* attached to it instead, which a virtual display may not have
/// at all, and silently returns nothing rather than the adapter's name.
fn adapter_friendly_name(device_name: &str) -> Option<String> {
    let mut index = 0u32;
    loop {
        let mut dd = DISPLAY_DEVICEW {
            cb: std::mem::size_of::<DISPLAY_DEVICEW>() as u32,
            ..Default::default()
        };
        let ok = unsafe { EnumDisplayDevicesW(PCWSTR::null(), index, &mut dd, 0) };
        if !ok.as_bool() {
            return None; // exhausted every adapter without a match
        }
        if decode_wide(&dd.DeviceName).eq_ignore_ascii_case(device_name) {
            return Some(decode_wide(&dd.DeviceString));
        }
        index += 1;
    }
}

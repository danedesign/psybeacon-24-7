//! PsychBeacon 24/7 — Windows host launcher.
//!
//! Module 2 (Windows Host Controller & VDD Layer): brings up a virtual
//! display via the Parsec VDD driver (see `vdd.rs`) and answers LAN
//! discovery broadcasts from the macOS client (module 1). Module 3
//! (capture.rs + encode.rs) exists and is independently verified, but not
//! wired into this default startup path yet — see the TODO marker below,
//! and capture.rs's module doc for why live capture specifically is
//! currently parked rather than integrated.
//!
//! The VDD *driver* is a proprietary, signed binary built by Parsec — it is
//! not vendored in this repo and this binary does not download or install
//! it. Install it manually first; see `docs/vdd-setup.md`. Without it,
//! `VddSession::start` fails and this launcher logs a warning and keeps
//! running the discovery responder anyway, so module 1 stays testable on a
//! machine that doesn't have the driver yet.

mod capture;
mod encode;
mod vdd;

use std::io;
use std::net::UdpSocket;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

const DISCOVERY_PORT: u16 = 43701;
const DISCOVERY_REQUEST: &str = "PSYBEACON_DISCOVER_V1";
const DISCOVERY_REPLY_PREFIX: &str = "PSYBEACON_HOST_V1:";

fn main() -> io::Result<()> {
    env_logger::init();

    // Maintenance escape hatch: a process that gets force-killed (e.g. a
    // dev session that didn't shut down cleanly) skips VddSession's Drop,
    // leaking its display on the shared adapter permanently — nothing else
    // will ever remove it. `--remove-index N` opens the adapter and removes
    // exactly that index directly, independent of whichever process added
    // it, then exits without touching anything else (no add, no capture,
    // no discovery responder).
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("--remove-index") => return remove_index_and_exit(&args),
        Some("--stream-test") => return stream_test_and_exit(&args),
        Some("--network-test") => return network_test_and_exit(&args),
        Some("--diagnose-capture") => {
            return capture::DesktopDuplicator::diagnose_all_existing_outputs(vdd::VDD_ADAPTER_NAME);
        }
        Some("--diagnose-settle") => return diagnose_settle_and_exit(&args),
        _ => {}
    }

    log::info!("PsychBeacon host launcher starting");

    let shutdown = Arc::new(AtomicBool::new(false));
    {
        let shutdown = shutdown.clone();
        ctrlc::set_handler(move || {
            log::info!("Shutdown requested (Ctrl+C)");
            shutdown.store(true, Ordering::Relaxed);
        })
        .expect("failed to install Ctrl+C handler");
    }

    // Must happen before VddSession::start opens the device — the driver
    // only reads these registry presets at adapter init. 3840x2160@120 is
    // the "4K/120Hz" target from the spec; add more (width, height, hz)
    // tuples here for additional selectable presets (up to 5 total).
    if let Err(e) = vdd::write_custom_resolutions(&[(3840, 2160, 120)]) {
        log::warn!(
            "VDD: couldn't write the custom-resolution registry preset ({e}). \
             Falling back to the driver's default EDID modes."
        );
    }

    let vdd_session = match vdd::VddSession::start(&vdd::VDD_ADAPTER_GUID) {
        Ok(session) => {
            log::info!(
                "VDD: virtual display {} is up and pinging (driver version: {:?})",
                session.display_index(),
                session.driver_version()
            );
            Some(session)
        }
        Err(e) => {
            log::warn!(
                "VDD: couldn't start a virtual display ({e}). Is the driver installed? \
                 See docs/vdd-setup.md. Continuing without it — discovery still works."
            );
            None
        }
    };

    // TODO(module 3/4): capture+encode (see `--stream-test` and
    // capture.rs/encode.rs, verified working standalone) deliberately isn't
    // wired in here yet — a real host shouldn't spend GPU/encode resources
    // with no client connected. Once module 4's connection handling exists,
    // start capturing/encoding when a client actually connects to this
    // `vdd_session`'s display, and stop (or drop the whole session) when it
    // disconnects, rather than running unconditionally from startup.

    run_discovery_responder(&shutdown)?;

    drop(vdd_session); // explicit: makes the teardown-on-exit ordering visible
    Ok(())
}

/// Answers UDP discovery broadcasts from macOS clients on the local subnet.
/// The client learns our address from the reply packet's source address, so
/// the payload only needs to carry an identifying token, not our own IP.
/// Polls `shutdown` between reads so Ctrl+C can unwind `main` (and drop the
/// `VddSession`) instead of killing the process mid-IOCTL.
fn run_discovery_responder(shutdown: &AtomicBool) -> io::Result<()> {
    let socket = UdpSocket::bind(("0.0.0.0", DISCOVERY_PORT))?;
    socket.set_broadcast(true)?;
    socket.set_read_timeout(Some(Duration::from_millis(500)))?;
    log::info!("Listening for discovery broadcasts on UDP :{DISCOVERY_PORT}");

    let hostname = hostname::get()
        .ok()
        .and_then(|h| h.into_string().ok())
        .unwrap_or_else(|| "psybeacon-host".to_string());
    let reply = format!("{DISCOVERY_REPLY_PREFIX}{hostname}");

    let mut buf = [0u8; 512];
    while !shutdown.load(Ordering::Relaxed) {
        let (len, src) = match socket.recv_from(&mut buf) {
            Ok(v) => v,
            Err(e) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut) => {
                continue
            }
            Err(e) => {
                log::warn!("recv_from failed: {e}");
                continue;
            }
        };

        if String::from_utf8_lossy(&buf[..len]).trim() != DISCOVERY_REQUEST {
            continue;
        }

        match socket.send_to(reply.as_bytes(), src) {
            Ok(_) => log::info!("Answered discovery request from {src}"),
            Err(e) => log::warn!("Failed to reply to {src}: {e}"),
        }
    }

    log::info!("Discovery responder shutting down");
    Ok(())
}

/// `--remove-index N`: opens the adapter and removes exactly display index
/// `N`, independent of whichever process (if any) added it, then exits.
/// Maintenance escape hatch — a process that gets force-killed skips
/// `VddSession`'s `Drop`, leaking its display on the shared adapter
/// permanently; nothing else will ever remove it otherwise.
fn remove_index_and_exit(args: &[String]) -> io::Result<()> {
    let index: u16 = args
        .get(2)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "usage: --remove-index N"))?
        .parse()
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "index must be a number"))?;

    let handle = vdd::open_device_handle(&vdd::VDD_ADAPTER_GUID)?;
    vdd::vdd_remove_display(handle, index);
    vdd::close_device_handle(handle);
    log::info!("VDD: removed display index {index}");
    Ok(())
}

/// `--stream-test SECONDS [OUTPUT]`: adds a display, captures and NVENC-
/// encodes it for `SECONDS`, then shuts down cleanly on its own — no Ctrl+C
/// needed, which is what makes this safe to run unattended (including by an
/// agent) without risking another orphaned display like the ones this
/// module's incident notes describe. `OUTPUT` defaults to `stream-test.h264`
/// (a local file, playable with `ffplay`/vlc) — pass a `udp://host:port` or
/// `srt://host:port` URL instead to test the actual network path.
fn stream_test_and_exit(args: &[String]) -> io::Result<()> {
    let seconds: u64 = args
        .get(2)
        .ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "usage: --stream-test SECONDS [OUTPUT]")
        })?
        .parse()
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "SECONDS must be a number"))?;
    let output = args
        .get(3)
        .map(String::as_str)
        .unwrap_or("stream-test.h264");

    log::info!("Stream test: running for {seconds}s, output={output}");

    let pre_existing_outputs =
        capture::DesktopDuplicator::snapshot_matching_outputs(vdd::VDD_ADAPTER_NAME)
            .unwrap_or_default();

    if let Err(e) = vdd::write_custom_resolutions(&[(3840, 2160, 120)]) {
        log::warn!("VDD: couldn't write the custom-resolution registry preset ({e}).");
    }

    let vdd_session = vdd::VddSession::start(&vdd::VDD_ADAPTER_GUID)?;
    log::info!("VDD: virtual display {} is up", vdd_session.display_index());

    let mut duplicator =
        capture::DesktopDuplicator::for_new_output(vdd::VDD_ADAPTER_NAME, &pre_existing_outputs)?;
    log::info!(
        "Capture: opened duplication on our newly-added output ({}x{})",
        duplicator.width(),
        duplicator.height()
    );

    const FPS: u32 = 30;
    let config = encode::NvencConfig {
        width: duplicator.width(),
        height: duplicator.height(),
        ..encode::NvencConfig::for_1080p(FPS)
    };
    let mut encoder = encode::NvencEncoder::spawn(&config, output)?;

    let frame_interval = Duration::from_millis(1000 / FPS as u64);
    let deadline = std::time::Instant::now() + Duration::from_secs(seconds);
    let mut frames_written = 0u32;

    while std::time::Instant::now() < deadline {
        match duplicator.capture_next_frame(frame_interval) {
            Ok(Some(frame)) => {
                encoder.write_frame(&frame.data)?;
                frames_written += 1;
            }
            Ok(None) => {} // static desktop this tick — nothing new to encode
            Err(e) => log::warn!("Capture: skipping a tick, AcquireNextFrame failed: {e}"),
        }
    }

    encoder.finish()?;
    log::info!("Stream test: wrote {frames_written} frames to {output}");

    drop(vdd_session); // explicit: removes the display before we exit
    Ok(())
}

/// `--diagnose-settle SECONDS`: adds a display, waits `SECONDS` untouched
/// (no retries, no recreate — just patience), then makes exactly one
/// capture attempt and reports the raw result before cleaning up.
///
/// Exists to test a specific theory in isolation: capture works fine on
/// `diagnose_all_existing_outputs`'s pre-existing, long-lived display but
/// fails with `ACCESS_LOST` on one this process just added (see
/// capture.rs's module doc comment for the full picture) — the one
/// difference that display has that a freshly-added one doesn't is time to
/// settle. `capture_next_frame`'s own retry loop only covers ~2.5s total;
/// this tests whether waiting substantially longer *before the first
/// attempt* succeeds where recreating duplication faster didn't.
fn diagnose_settle_and_exit(args: &[String]) -> io::Result<()> {
    let seconds: u64 = args
        .get(2)
        .ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "usage: --diagnose-settle SECONDS")
        })?
        .parse()
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "SECONDS must be a number"))?;

    let pre_existing_outputs =
        capture::DesktopDuplicator::snapshot_matching_outputs(vdd::VDD_ADAPTER_NAME)
            .unwrap_or_default();
    let vdd_session = vdd::VddSession::start(&vdd::VDD_ADAPTER_GUID)?;
    log::warn!("Diagnostic: display {} added, waiting {seconds}s before touching it", vdd_session.display_index());

    std::thread::sleep(Duration::from_secs(seconds));

    let mut duplicator =
        capture::DesktopDuplicator::for_new_output(vdd::VDD_ADAPTER_NAME, &pre_existing_outputs)?;
    log::warn!("Diagnostic: opened duplication after the wait, attempting one capture");

    match duplicator.capture_next_frame(Duration::from_secs(2)) {
        Ok(Some(frame)) => log::warn!(
            "Diagnostic: SUCCESS — captured a {}x{} frame after waiting {seconds}s",
            frame.width,
            frame.height
        ),
        Ok(None) => log::warn!("Diagnostic: opened fine, no new frame within 2s (static desktop)"),
        Err(e) => log::warn!("Diagnostic: FAILED even after waiting {seconds}s: {e}"),
    }

    drop(vdd_session);
    Ok(())
}

/// `--network-test SECONDS OUTPUT_URL [SOURCE_BGRA_FILE]`: feeds a static
/// frame through NVENC to `OUTPUT_URL` repeatedly for `SECONDS`, at 30fps.
/// Exists to test the encode+network path in isolation from live capture,
/// which is currently broken — see capture.rs's module doc comment.
/// `SOURCE_BGRA_FILE` (default `raw-frame.bgra`, not committed — generate
/// your own, e.g. `tail -c +55 capture-test.bmp > raw-frame.bgra` from a
/// `--stream-test` capture, or any raw BGRA8 dump at that exact size) must
/// be tightly-packed BGRA8 at exactly 1920×1080×4 bytes, the same format
/// `capture.rs` produces. Verify receipt on the other end with e.g.
/// `ffplay udp://0.0.0.0:PORT` or `ffprobe -i udp://0.0.0.0:PORT`.
fn network_test_and_exit(args: &[String]) -> io::Result<()> {
    let usage = "usage: --network-test SECONDS OUTPUT_URL [SOURCE_BGRA_FILE]";
    let seconds: u64 = args
        .get(2)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, usage))?
        .parse()
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "SECONDS must be a number"))?;
    let output = args
        .get(3)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, usage))?;
    let source_path = args.get(4).map(String::as_str).unwrap_or("raw-frame.bgra");

    const WIDTH: u32 = 1920;
    const HEIGHT: u32 = 1080;
    const FPS: u32 = 30;
    let expected_len = (WIDTH * HEIGHT * 4) as usize;

    let frame_data = std::fs::read(source_path)?;
    if frame_data.len() != expected_len {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "{source_path} is {} bytes, expected exactly {expected_len} \
                 ({WIDTH}x{HEIGHT} BGRA8) — pass a matching file explicitly if \
                 your source frame isn't 1080p",
                frame_data.len()
            ),
        ));
    }

    log::info!("Network test: streaming {source_path} to {output} for {seconds}s");

    let config = encode::NvencConfig {
        width: WIDTH,
        height: HEIGHT,
        ..encode::NvencConfig::for_1080p(FPS)
    };
    let mut encoder = encode::NvencEncoder::spawn(&config, output)?;

    let frame_interval = Duration::from_millis(1000 / FPS as u64);
    let deadline = std::time::Instant::now() + Duration::from_secs(seconds);
    let mut frames_written = 0u32;

    while std::time::Instant::now() < deadline {
        let tick_start = std::time::Instant::now();
        encoder.write_frame(&frame_data)?;
        frames_written += 1;
        if let Some(remaining) = frame_interval.checked_sub(tick_start.elapsed()) {
            std::thread::sleep(remaining);
        }
    }

    encoder.finish()?;
    log::info!("Network test: sent {frames_written} frames to {output}");
    Ok(())
}

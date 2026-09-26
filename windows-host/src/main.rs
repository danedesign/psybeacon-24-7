//! PsychBeacon 24/7 — Windows host launcher.
//!
//! Answers module 1's LAN discovery broadcasts, and — once a client follows
//! up with a `PSYBEACON_START_STREAM_V1:<port>` request over that same
//! socket — brings up a virtual display (`vdd.rs`), captures it
//! (`capture.rs`), and NVENC-encodes it to that client over UDP
//! (`encode.rs`), all per `run_stream_session`. Each stream is fully
//! self-contained: its own display, added on request and removed when the
//! stream ends, not something kept running at launch regardless of whether
//! a client wants it.
//!
//! The VDD *driver* is a proprietary, signed binary built by Parsec — it is
//! not vendored in this repo and this binary does not download or install
//! it. Install it manually first; see `docs/vdd-setup.md`. Without it, a
//! start-stream request fails (logged, not fatal) and the discovery
//! responder keeps running regardless, so module 1 stays testable on a
//! machine that doesn't have the driver yet.

mod capture;
mod clipboard;
mod encode;
mod sidecar;
mod vdd;

use std::io;
use std::net::UdpSocket;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;

const DISCOVERY_PORT: u16 = 43701;
const DISCOVERY_REQUEST: &str = "PSYBEACON_DISCOVER_V1";
const DISCOVERY_REPLY_PREFIX: &str = "PSYBEACON_HOST_V1:";
/// Sent by a client, over the same discovery socket, after it's resolved a
/// `HostRoute` and wants the actual video stream:
/// `PSYBEACON_START_STREAM_V1:<basePort>:<count>`, where `<basePort>` is the
/// first UDP port on the *client* to stream to (display `i` streams to
/// `basePort + i`) and `<count>` is how many virtual displays to add. The
/// client's address comes from the packet's source, same non-spoofable
/// pattern as discovery itself — the message never needs to carry an IP.
/// `:<count>` is optional and defaults to 1, for a plain single-display
/// request.
const START_STREAM_PREFIX: &str = "PSYBEACON_START_STREAM_V1:";
/// Sent back to the client, over the discovery socket, once every requested
/// display has actually been added and its real bounds are known (a JSON
/// array of `DisplayInfo`) — *before* any frames start flowing, so the
/// client can build its N windows first. Bounds can't be predicted ahead of
/// time (the driver, not the client, decides where each display lands on
/// the virtual desktop), so this reply exists instead of the client just
/// guessing from the custom-resolution preset it can't see either.
const STREAM_INFO_PREFIX: &str = "PSYBEACON_STREAM_INFO_V1:";
/// v1 sanity cap — each display needs its own settle delay (~2s) added
/// sequentially before streaming can begin, so this bounds worst-case
/// negotiation latency as much as it bounds resource usage.
const MAX_DISPLAY_COUNT: u16 = 4;
const STREAM_FPS: u32 = 30;
/// Fixed, not negotiated: unlike the video port (client-specified, since
/// the *host* connects out to it), the sidecar is a server the host runs —
/// the client just connects to `ws://<host>:<SIDECAR_PORT + display index>`
/// once it already knows the host's address from discovery and the display's
/// index from the stream-info manifest, no separate protocol message needed.
const SIDECAR_PORT: u16 = 43703;

/// One entry in the `STREAM_INFO_PREFIX` manifest — everything the client
/// needs to stand up a window for this specific virtual display: which
/// ports to use, and where this display sits on the host's virtual desktop
/// (so a click at the client's local origin maps to this display's corner,
/// not the whole virtual desktop's — see `sidecar.rs`'s `DisplayBounds`).
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DisplayInfo {
    index: u16,
    stream_port: u16,
    sidecar_port: u16,
    width: u32,
    height: u32,
    left: i32,
    top: i32,
}

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

    // No VddSession here at startup, deliberately: each start-stream
    // request (see run_stream_session, spawned from run_discovery_responder
    // below) adds its own display on demand and removes it when the stream
    // ends. Module 2's original always-on-at-launch VddSession moved into
    // `--stream-test`, which still wants exactly that behavior for
    // standalone testing.
    run_discovery_responder(shutdown)?;
    Ok(())
}

/// Answers UDP discovery broadcasts from macOS clients on the local subnet,
/// and start-stream requests that follow. The client learns our address
/// from the reply packet's source address, so neither message needs to
/// carry an IP. Polls `shutdown` between reads so Ctrl+C can unwind `main`
/// cleanly; a spawned stream session gets its own clone of `shutdown` so
/// it, too, stops (and drops its `VddSession`, removing its display) on
/// Ctrl+C rather than being abandoned when the process exits.
fn run_discovery_responder(shutdown: Arc<AtomicBool>) -> io::Result<()> {
    let socket = UdpSocket::bind(("0.0.0.0", DISCOVERY_PORT))?;
    socket.set_broadcast(true)?;
    socket.set_read_timeout(Some(Duration::from_millis(500)))?;
    log::info!("Listening for discovery broadcasts on UDP :{DISCOVERY_PORT}");

    let hostname = hostname::get()
        .ok()
        .and_then(|h| h.into_string().ok())
        .unwrap_or_else(|| "psybeacon-host".to_string());
    let reply = format!("{DISCOVERY_REPLY_PREFIX}{hostname}");

    // Guards against overlapping stream sessions (e.g. a retried request,
    // or a second client) stepping on each other — VddSession::start's
    // settle delay and capture's before/after diffing both assume they're
    // the only thing adding/removing displays on the adapter at the time.
    // v1 scope: one stream at a time; a request while already streaming is
    // logged and ignored rather than queued or replacing the active one.
    let streaming = Arc::new(AtomicBool::new(false));

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

        let message = String::from_utf8_lossy(&buf[..len]);
        let message = message.trim();

        if message == DISCOVERY_REQUEST {
            match socket.send_to(reply.as_bytes(), src) {
                Ok(_) => log::info!("Answered discovery request from {src}"),
                Err(e) => log::warn!("Failed to reply to {src}: {e}"),
            }
        } else if let Some(rest) = message.strip_prefix(START_STREAM_PREFIX) {
            let mut parts = rest.splitn(2, ':');
            let Some(Ok(base_port)) = parts.next().map(str::parse::<u16>) else {
                log::warn!("Start-stream request from {src} has an invalid port: {rest:?}");
                continue;
            };
            let display_count: u16 = match parts.next() {
                Some(count_str) => match count_str.parse() {
                    Ok(n) => n,
                    Err(_) => {
                        log::warn!("Start-stream request from {src} has an invalid count: {count_str:?}");
                        continue;
                    }
                },
                None => 1,
            };
            let display_count = display_count.clamp(1, MAX_DISPLAY_COUNT);

            if streaming.swap(true, Ordering::SeqCst) {
                log::warn!(
                    "Start-stream request from {src} ignored — a session is already active \
                     (v1 supports one client at a time)"
                );
                continue;
            }

            let Ok(reply_socket) = socket.try_clone() else {
                log::warn!("Start-stream request from {src}: couldn't clone the socket to reply on");
                streaming.store(false, Ordering::SeqCst);
                continue;
            };
            let shutdown = shutdown.clone();
            let streaming = streaming.clone();
            std::thread::spawn(move || {
                run_multi_stream_session(&shutdown, &reply_socket, src, base_port, display_count);
                streaming.store(false, Ordering::SeqCst);
            });
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

/// Adds `display_count` virtual displays sequentially, then opens capture
/// interfaces only after all topology changes are complete. Replies to the
/// client with the `STREAM_INFO_PREFIX` manifest once their real bounds are
/// known, then runs one capture/encode/sidecar loop per display concurrently
/// until the client disconnects or `shutdown` fires. Multi-display window
/// orchestration's host half:
/// each display gets its own stream port (`base_port + index`) and sidecar
/// port (`SIDECAR_PORT + index`) so the client can run N independent
/// windows, each with its own decoder, renderer, and input channel.
fn run_multi_stream_session(
    shutdown: &AtomicBool,
    reply_socket: &UdpSocket,
    client_src: std::net::SocketAddr,
    base_port: u16,
    display_count: u16,
) {
    log::info!("Multi-stream: request from {client_src} for {display_count} display(s)");

    if let Err(e) = vdd::write_custom_resolutions(&[(3840, 2160, 120)]) {
        log::warn!("VDD: couldn't write the custom-resolution registry preset ({e}).");
    }

    let mut added_displays = Vec::new();
    for index in 0..display_count {
        let pre_existing_outputs =
            capture::DesktopDuplicator::snapshot_matching_outputs(vdd::VDD_ADAPTER_NAME)
                .unwrap_or_default();

        let vdd_session = match vdd::VddSession::start(&vdd::VDD_ADAPTER_GUID) {
            Ok(s) => s,
            Err(e) => {
                log::warn!(
                    "Multi-stream: couldn't add display {index} ({e}) — stopping at {index} of {display_count}"
                );
                break;
            }
        };

        let output_name =
            match capture::DesktopDuplicator::new_output_name(
                vdd::VDD_ADAPTER_NAME,
                &pre_existing_outputs,
            ) {
                Ok(name) => name,
                Err(e) => {
                    log::warn!(
                        "Multi-stream: couldn't open capture for display {index} ({e}) — \
                         stopping at {index} of {display_count}"
                    );
                    break;
                }
            };

        log::info!(
            "Multi-stream: display {index} virtual display {} is up (driver version: {:?})",
            vdd_session.display_index(),
            vdd_session.driver_version()
        );

        added_displays.push((index, vdd_session, output_name));
    }

    let mut sessions = Vec::new();
    for (index, vdd_session, output_name) in added_displays {
        let duplicator = match capture::DesktopDuplicator::for_output_name(
            vdd::VDD_ADAPTER_NAME,
            &output_name,
        ) {
            Ok(d) => d,
            Err(e) => {
                log::warn!(
                    "Multi-stream: couldn't open capture for display {index} ({e}) — skipping it"
                );
                drop(vdd_session);
                continue;
            }
        };

        let bounds = duplicator.bounds();
        let info = DisplayInfo {
            index,
            stream_port: base_port + index,
            sidecar_port: SIDECAR_PORT + index,
            width: duplicator.width(),
            height: duplicator.height(),
            left: bounds.left,
            top: bounds.top,
        };
        log::info!(
            "Multi-stream: display {index} up: {}x{} at ({}, {}), stream_port={}, sidecar_port={}",
            info.width, info.height, info.left, info.top, info.stream_port, info.sidecar_port
        );
        sessions.push((vdd_session, duplicator, info));
    }

    if sessions.is_empty() {
        log::warn!("Multi-stream: no displays could be added for {client_src}, aborting");
        return;
    }

    let manifest: Vec<&DisplayInfo> = sessions.iter().map(|(_, _, info)| info).collect();
    match serde_json::to_string(&manifest) {
        Ok(json) => {
            let message = format!("{STREAM_INFO_PREFIX}{json}");
            if let Err(e) = reply_socket.send_to(message.as_bytes(), client_src) {
                log::warn!("Multi-stream: couldn't send the manifest to {client_src}: {e}");
            }
        }
        Err(e) => log::warn!("Multi-stream: couldn't serialize the manifest: {e}"),
    }

    let client_ip = client_src.ip();
    let client_disconnected = Arc::new(AtomicBool::new(false));
    std::thread::scope(|scope| {
        let handles: Vec<_> = sessions
            .into_iter()
            .map(|(vdd_session, duplicator, info)| {
                let target = format!("udp://{client_ip}:{}", info.stream_port);
                let sidecar_port = info.sidecar_port;
                let client_disconnected = client_disconnected.clone();
                scope.spawn(move || {
                    run_display_stream(
                        shutdown,
                        client_disconnected,
                        vdd_session,
                        duplicator,
                        target,
                        sidecar_port,
                    );
                })
            })
            .collect();

        for handle in handles {
            if handle.join().is_err() {
                log::warn!("Multi-stream: a display worker thread panicked");
            }
        }
    });

    log::info!("Multi-stream: session for {client_src} ended");
}

/// One display's capture → encode → network loop, plus its own sidecar
/// input server, running until the client disconnects or `shutdown` fires. Split out of the old
/// single-display `run_stream_session` so `run_multi_stream_session` can run
/// several of these concurrently, each on its own already-added display.
fn run_display_stream(
    shutdown: &AtomicBool,
    client_disconnected: Arc<AtomicBool>,
    vdd_session: vdd::VddSession,
    mut duplicator: capture::DesktopDuplicator,
    target: String,
    sidecar_port: u16,
) {
    log::info!(
        "Stream: capturing {}x{}, encoding to {target}",
        duplicator.width(),
        duplicator.height()
    );

    let config = encode::NvencConfig {
        width: duplicator.width(),
        height: duplicator.height(),
        ..encode::NvencConfig::for_1080p(STREAM_FPS)
    };
    let mut encoder = match encode::NvencEncoder::spawn(&config, &target) {
        Ok(e) => e,
        Err(e) => {
            log::warn!("Stream: couldn't start the encoder ({e}) — aborting stream for {target}");
            client_disconnected.store(true, Ordering::SeqCst);
            return;
        }
    };

    // Tied to this stream, not the global `shutdown` — the sidecar's TCP
    // listener has to release `sidecar_port` before a later session can bind it.
    let sidecar_shutdown = Arc::new(AtomicBool::new(false));
    let sidecar_thread = {
        let bounds = duplicator.bounds();
        let sidecar_shutdown = sidecar_shutdown.clone();
        let client_disconnected = client_disconnected.clone();
        std::thread::spawn(move || {
            if let Err(e) = sidecar::run_sidecar_server(
                sidecar_port,
                bounds,
                &sidecar_shutdown,
                &client_disconnected,
            ) {
                log::warn!("Sidecar: server failed on port {sidecar_port}: {e}");
                client_disconnected.store(true, Ordering::SeqCst);
            }
        })
    };

    let frame_interval = Duration::from_millis(1000 / STREAM_FPS as u64);
    while !shutdown.load(Ordering::Relaxed) && !client_disconnected.load(Ordering::Relaxed) {
        match duplicator.capture_next_frame(frame_interval) {
            Ok(Some(frame)) => {
                if let Err(e) = encoder.write_frame(&frame.data) {
                    log::warn!("Stream: write_frame failed, stopping: {e}");
                    break;
                }
            }
            Ok(None) => {} // static desktop this tick — nothing new to encode
            Err(e) => log::warn!("Stream: capture tick failed: {e}"),
        }
    }

    if let Err(e) = encoder.finish() {
        log::warn!("Stream: encoder didn't shut down cleanly: {e}");
    }

    if client_disconnected.load(Ordering::Relaxed) {
        log::info!("Stream: client disconnected; stopping session for {target}");
    }

    sidecar_shutdown.store(true, Ordering::SeqCst);
    if sidecar_thread.join().is_err() {
        log::warn!("Sidecar: server thread panicked");
    }

    drop(vdd_session); // explicit: removes the display before this thread ends
    log::info!("Stream: stopped for {target}");
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

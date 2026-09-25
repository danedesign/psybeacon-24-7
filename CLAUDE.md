# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project overview

PsychBeacon 24/7 is a custom, high-performance remote desktop tool for using a macOS client to control a remote Windows host — ultra-low-latency, high-refresh-rate (120Hz+) streaming, seamless virtual multi-display management, and shared rich clipboard history, replicating premium Parsec/Moonlight features without a subscription.

This repo is very early-stage: module 1 (network discovery), most of module 2 (host launcher + VDD control), and half of module 3 (capture + NVENC encode, independently verified but not currently working *together* — see module 3 below) exist. Module 4 is architecture, not code.

## Repository layout

Two independent components, each with its own toolchain — there is no shared build system or workspace tying them together:

- `windows-host/` — Rust crate; runs on the Windows machine being controlled.
- `mac-client/` — Swift package; runs on the macOS client. **Requires macOS/Xcode to build** — it cannot be built or type-checked from this Windows checkout.

## Commands

**windows-host** (Rust, from `windows-host/`):
```
cargo check          # type-check
cargo run             # build and run the launcher
cargo build --release
```
No test suite exists yet. **Run elevated (Administrator)** once the real
driver is installed — the VDD registry write and device open both need it;
without elevation they fail with a logged warning and the launcher keeps
running in a degraded mode (discovery still works). See
[`windows-host/docs/vdd-setup.md`](windows-host/docs/vdd-setup.md).

**mac-client** (Swift, from `mac-client/`, on macOS only):
```
swift build
swift run
```
Not yet verified to build — write against `Package.swift` (SwiftPM, macOS 13+ target) and confirm on an actual Mac.

## Architecture

Four modules, in the order data flows through them at connection time:

1. **Network discovery & routing** (`mac-client/Sources/PsychBeaconClient/NetworkDiscovery.swift`, answered by `windows-host/src/main.rs`) — *implemented*. On launch, the client sends a UDP broadcast (`PSYBEACON_DISCOVER_V1` on port 43701) via a raw BSD socket (Network.framework doesn't expose `SO_BROADCAST`, so it can't reliably send to a broadcast address). If the host answers, the client uses the reply packet's *source address* as the host IP — the reply payload is just an identifying token, not the IP — and connects directly (LAN path, lowest latency). If nothing answers within the timeout, the client shells out to the local `tailscale` CLI (`status --json`) to find the host's 100.x.x.x mesh IP and routes over the Tailscale/WireGuard tunnel instead. This module's job is to hand later modules an opaque `HostRoute` (`.lan` or `.tailscale`) — modules 3/4 should not need to know which path was chosen.

2. **Windows host controller & virtual display driver** (`windows-host/src/vdd.rs`) — *implemented and verified end-to-end against real driver v0.45.0.0 on 2026-09-22* (registry write → device open → add-display → heartbeat → version query, all confirmed working elevated). Not bundling the driver itself. This is a Rust port of [nomi-san/parsec-vdd](https://github.com/nomi-san/parsec-vdd)'s `core/parsec-vdd.h` (verified against the real header + its reverse-engineered docs, not assumed from the original spec — two corrections came out of that):
   - `write_custom_resolutions` writes up to 5 `(width, height, hz)` presets to `HKLM\SOFTWARE\Parsec\vdd\<slot>` (REG_DWORD `w`/`h`/`hz`) — this registry mechanism is real, but must run *before* the device is opened; the driver only reads it at adapter init, not per-display.
   - `VddSession::start` opens the device (`VDD_ADAPTER_GUID`, resolved via SetupAPI — there's no fixed symbolic link path), adds a display via IOCTL, and spawns a heartbeat thread pinging every **50ms**. The original spec's "1-second heartbeat" had it backwards: 1 second is the driver's watchdog *timeout* — if pings stop for ~1s the driver unplugs every virtual display, so the actual requirement (per upstream docs) is pings every ~100ms or tighter.
   - `VddSession` removes the display and closes the handle on `Drop`, which is what stands in for "clean up on client disconnect" until module 3 exists to actually own a per-client connection and drop the session when it ends.
   - The driver binary itself (`mm.inf`/`mm.dll`/`mm.cat`) is proprietary (built and signed by Parsec, not part of the MIT-licensed wrapper this was ported from) and is **not vendored here** — installing it is a manual prerequisite, see [`windows-host/docs/vdd-setup.md`](windows-host/docs/vdd-setup.md). Without it, `main.rs` logs a warning and keeps running in a degraded mode rather than failing outright.

3. **Capture & video streaming** (`windows-host/src/capture.rs`, `windows-host/src/encode.rs`) — *each half independently verified working; integration currently broken, parked, see below*.
   - `capture.rs`: DXGI Desktop Duplication, scoped to specifically the display this process just added (a before/after snapshot diff — see `for_new_output`'s doc comment — not name-matching, since the adapter is shared with whatever else is using it, e.g. a live Parsec session). Not zero-copy — does a GPU→CPU readback (`CopyResource` to a staging texture, then `Map`) so frames can go over a pipe to FFmpeg; true zero-copy would need direct NVENC SDK integration or an in-process `AVHWFramesContext`.
   - `encode.rs`: spawns `ffmpeg` as a subprocess, feeds it raw BGRA frames over stdin, NVENC-encodes to a local file or a `udp://`/`srt://` URL. Verified against real hardware (RTX 4080) with both a synthetic test pattern and actual captured bytes. The original spec's `preset=ultrafast`/`tune=zerolatency` are x264 flag *names* that don't exist for `h264_nvenc` — real equivalents (verified via `ffmpeg -h encoder=h264_nvenc` on this machine, not assumed) are `-preset p1`, `-tune ull`, plus a distinct `-zerolatency 1` boolean. Also: this machine's first FFmpeg install (Gyan 9.0.2) was compiled against a newer NVENC SDK than the installed driver supports and failed outright — fixed by installing an older build (BtbN 7.1.5) rather than touching the GPU driver.
   - **Currently broken**: wiring live capture into the encode loop hits `DXGI_ERROR_ACCESS_LOST` ("the keyed mutex was abandoned") persistently — survives recreating duplication, re-asserting extended-desktop topology, elevation (ruled out, tested both ways), and 2.5s of retries. `capture.rs`'s module doc comment has the full incident writeup and leading theory (a keyed-mutex lock left abandoned by an earlier debugging session that force-killed processes mid-capture, poisoning the shared adapter). Candidate fixes (restarting `dwm.exe`, cycling the adapter, a reboot) all risk disrupting whatever's currently rendering on this shared, live machine, so none have been attempted — deliberately left for the machine's owner to do at a time of their choosing. Don't re-attempt those without reading that comment first.
   - `main.rs --stream-test SECONDS [OUTPUT]` runs this whole pipeline standalone (add display → capture → encode → clean shutdown, no Ctrl+C needed) for exactly this kind of testing; `--remove-index N` removes a specific stray display directly, for cleaning up anything a force-kill orphaned. Prefer `--stream-test` over ad hoc testing that needs an external Ctrl+C or force-kill — see `vdd.rs`'s incident notes for what killing a process mid-session cleanup cost last time.

4. **macOS client & sidecar portal** (not started) — planned: VideoToolbox for hardware decode, rendered via Metal (or SDL2) adapting to ProMotion refresh rates; one native window per virtual display the host registers; a parallel WebSocket "sidecar" channel carrying keyboard/mouse input and bidirectional clipboard sync (NSPasteboard ↔ Win32 clipboard).

## Conventions established so far

- Discovery protocol constants (port `43701`, request/reply tokens) are duplicated in both `NetworkDiscovery.swift` and `windows-host/src/main.rs`. If they diverge, the handshake silently stops working — there's no shared schema file yet, so keep them in sync by hand until one exists.
- `windows-host` TODOs are marked `// TODO(module N): ...` pointing at the module in this file that will fill them in — grep for `TODO(module` before adding new host functionality to see what's already flagged.
- `vdd.rs` is a deliberately faithful port of upstream's C header/docs (constants, IOCTL codes, function shapes match 1:1) rather than a redesign — if the driver's behavior seems off, diff against the upstream source linked in that file's doc comment before assuming the port is wrong. **"Faithful" means matching upstream's actual choices, not substituting equivalent-looking ones** — the access-rights bug below happened because `FILE_GENERIC_READ | FILE_GENERIC_WRITE` looked like a reasonable stand-in for upstream's plain `GENERIC_READ | GENERIC_WRITE` and wasn't. The known deviations from the original PsychBeacon spec text (heartbeat cadence; the registry step is real but lives in the README's "Known Limitations" section rather than the core API docs) plus two real bugs found getting this working against actual hardware (wrong `CreateFileW` access rights; an `HRESULT`-vs-`WIN32_ERROR` comparison that silently masked the first bug's real error) are called out in `vdd.rs`'s module doc comment — treat that comment, not the original spec prose, as authoritative for module 2.

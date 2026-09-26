# Project status

Last updated: 2026-09-26

PsychBeacon is a macOS client for viewing and controlling a Windows desktop
through a Parsec virtual display. This file tracks the current working state
and next steps. See [`TIMELINE.md`](TIMELINE.md) for the chronological history
and [`AGENTS.md`](AGENTS.md) for architecture details.

## Current status

**Single-display streaming and basic remote input work end to end over
Tailscale.** The user reports that the latest client is substantially more
stable than the previous version. Trackpad scrolling still does not work, and
video playback still feels sluggish; both remain on the fix list.

**Display issue under investigation:** the user reported the Windows physical
main display blinking while streaming. It stopped after the old host process
was cancelled. At 30 fps, the latest fresh Mac connection negotiated display 0,
received H.264, and connected the input sidecar. The user has not yet confirmed
whether the physical display remains stable during streaming.

Latest changes are in commit `8d6ffe9` (`Add text clipboard sync over
sidecar`). Rust compilation passes; Swift compilation and live clipboard
verification remain.

## Verified

- Windows host adds a Parsec VDD display, captures it, and NVENC-encodes the
  desktop.
- Mac client discovers the host through Tailscale, negotiates one display,
  receives H.264, decodes it with VideoToolbox, and renders it with Metal.
- Basic mouse and keyboard input reaches the Windows host through the WebSocket
  sidecar.
- Mac `swift build` succeeds after the latest client changes.
- Windows `cargo check` succeeds after the latest host changes.
- The user restarted the updated Windows host successfully; it reported
  listening on UDP port 43701.

## Implemented, needs live verification

- Host video rate is set to 30 fps while investigating the physical-display
  blinking report.
- Mac rendering is requested when a decoded frame arrives; rapid mouse
  movements are coalesced to reduce stale cursor events.
- Host accumulates precise trackpad deltas before sending Windows wheel events;
  the user reports scrolling still does not work, so the path needs diagnosis.
- Pinch magnification is mapped to Windows Ctrl+wheel zoom.

## Open work

1. **Physical display blinking:** idle host is stable; the latest 30 fps stream started successfully; confirm monitor stability
   and stop with Ctrl+C if blinking returns. The cause is not confirmed yet.
2. **Video performance:** playback still feels sluggish. Investigate capture,
   encoding, UDP receive, decode, and render pacing before increasing frame rate.
3. **Trackpad scrolling:** still not working after host-side wheel-delta
   accumulation; diagnose the macOS event, WebSocket payload, and Windows input
   path with targeted logging.
4. **Pinch zoom:** implemented as Ctrl+wheel, but not yet confirmed in a target
   Windows application.
5. **Multi-display:** code supports multiple displays, but only one display has
   been exercised in the user's live setup.
6. **Clipboard sync:** text-only bidirectional sync is implemented over the
   existing sidecar, limited to display 0 and 1 MiB. Needs live Mac/Windows
   verification.
7. **Video transport resilience:** raw H.264 over UDP has no sequence numbers
   or retransmission; packet loss can damage decoding until reconnect.

## Current connection note

The LAN can reply from a stale host address. For this setup, launch the Mac
client with `PSYBEACON_TARGET_HOST=desktop-1aachgk` to route directly over
Tailscale and avoid the stale LAN discovery response.

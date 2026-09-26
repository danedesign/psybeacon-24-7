# Project status

Last updated: 2026-09-26

PsychBeacon is a macOS client for viewing and controlling a Windows desktop
through a Parsec virtual display. This file tracks the current working state
and next steps. See [`TIMELINE.md`](TIMELINE.md) for the chronological history
and [`AGENTS.md`](AGENTS.md) for architecture details.

## Current status

**Single-display streaming and basic remote input work end to end over
Tailscale.** The user reports that the latest client is substantially more
stable than the previous version. Trackpad scrolling still does not work.

**Display issue under investigation:** the user reported the Windows physical
main display blinking while streaming. It stopped after the old host process
was cancelled. At 30 fps, the latest fresh Mac connection negotiated display 0,
received H.264, and connected the input sidecar. The user has not yet confirmed
whether the physical display remains stable during streaming.

Latest implementation changes are in commit `35990c0` (`Improve remote display
streaming and input`), pushed to `origin/master`.

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

- Host video rate is set to 30 fps while
  investigating the physical-display blinking report.
- Mac rendering is requested when a decoded frame arrives; rapid mouse
  movements are coalesced to reduce stale cursor events.
- Trackpad scrolling scales precise deltas before sending them to Windows.
- Pinch magnification is mapped to Windows Ctrl+wheel zoom.

## Open work

1. **Physical display blinking:** idle host is stable; the latest 30 fps stream started successfully; confirm monitor stability
   and stop with Ctrl+C if blinking returns. The cause is not confirmed yet.
2. **Session cleanup:** stop FFmpeg and remove the virtual display when the Mac
   disconnects. Host-side cleanup is implemented: the WebSocket disconnect
   ends FFmpeg and releases the VDD display; a stream with no sidecar connection
   is stopped after 15 seconds. Needs a live disconnect check.
3. **Trackpad scrolling:** host now accumulates small trackpad deltas and sends
   full Windows wheel detents. Needs a host restart and live Mac verification.
4. **Pinch zoom:** implemented as Ctrl+wheel, but not yet confirmed in a target
   Windows application.
5. **Multi-display:** code supports multiple displays, but only one display has
   been exercised in the user's live setup.
6. **Clipboard sync:** not implemented.
7. **Video transport resilience:** raw H.264 over UDP has no sequence numbers
   or retransmission; packet loss can damage decoding until reconnect.

## Current connection note

The LAN can reply from a stale host address. For this setup, launch the Mac
client with `PSYBEACON_TARGET_HOST=desktop-1aachgk` to route directly over
Tailscale and avoid the stale LAN discovery response.

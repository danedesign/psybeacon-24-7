# Project status

Last updated: 2026-09-27

PsychBeacon is a macOS client for viewing and controlling a Windows desktop
through a Parsec virtual display. This file tracks current work and priorities.
See [`TIMELINE.md`](TIMELINE.md) for chronological history and [`AGENTS.md`](AGENTS.md)
for architecture details.

## Current status

The end-to-end stream works over Tailscale. The host and Mac client support
multiple independent displays, with each stream configured for 30 fps. The
user reports the physical Windows display no longer blinks after reloading the
host with the capture-recovery fix. Video still feels sluggish and trackpad
scrolling still does not work.

## Verified

- Host discovery through LAN/Tailscale, virtual-display creation, capture,
  NVENC encoding, Mac H.264 receive/decode, and Metal rendering.
- Basic mouse and keyboard input through the WebSocket sidecar.
- Bidirectional plain-text clipboard sync through the first display's sidecar.
- Host and client compile after multi-display startup and capture-recovery
  changes (`8978c50`, `b9b6962`).
- The physical Windows display is stable after reloading the latest host, per
  the user's report.
- A tracked Administrator launcher and `--preflight` command now check Cargo,
  FFmpeg/NVENC, the Parsec VDD driver, and required ports before listening;
  the new elevated launcher has not yet been run live.
- The Mac client now has a computer picker that lists online Tailscale peers
  answering PsychBeacon discovery, plus a display-count selector. The Swift
  build passes; interactive GUI discovery and connection still need a live run.
- Mac disconnect is now explicit: closing any stream window or using the
  floating Disconnect control should close every stream receiver and sidecar,
  return to the picker, and allow a new connection. This lifecycle change needs
  a live reconnect check.
- Windows now has an Automatic LocalSystem service supervisor. It launches a
  SYSTEM worker in the active console session, restarts it after a session
  change or unexpected exit, and keeps DXGI capture out of Session 0. The
  installer scopes inbound host ports to Tailscale IPv4 addresses and migrates
  the prior logon task. Build and script parsing pass, but install, reboot,
  account switching, and lock-screen capture still need live verification on
  the host PC. This design serves only the active console session, not multiple
  signed-in desktops simultaneously. Tailscale scoping is not client
  authentication; tailnet ACLs must restrict trusted peers.
- Both requested display streams are configured for 30 fps each. Actual frame
  delivery has not been measured independently per display.

## Main development priorities

1. **Video performance and refresh rate.** Playback feels sluggish. Measure
   capture, GPU readback, NVENC, UDP delivery, decode, and render pacing. Fix
   the bottleneck before raising the current 30 fps target; then evaluate
   60/120 fps and two-stream load.
2. **Trackpad input.** Diagnose why scroll events fail end to end, from macOS
   gesture capture through the WebSocket and Windows wheel injection. Confirm
   pinch-to-zoom in a real Windows application.
3. **Multi-display reliability.** Verify independent video and input on each
   display, reconnect behavior, and cleanup of virtual displays after normal
   disconnects and capture failures. Current two-display support has just been
   exercised in the user's setup; longer stability and per-display input still
   need confirmation.
4. **Transport resilience.** Video is raw H.264 over UDP without packet
   sequencing or retransmission. Add loss detection and a recovery strategy so
   one lost packet does not leave decoding damaged until reconnect.
5. **Clipboard scope.** Text sync works in both directions. Rich content such
   as images and files, plus clipboard history, remains future work.
6. **Security and product operation.** Add explicit session authentication
   before exposing the host beyond a restricted private tailnet; package the
   Mac picker as a normal app; then improve setup, configuration, host status,
   and diagnostics for routine use. The new Windows service has not yet been
   installed or exercised through reboot, lock, and account switching.

## Current run notes

- Both stream targets are configured for 30 fps; this is a cap/target, not a
  measured guarantee of delivered frames.
- For this setup, use `PSYBEACON_TARGET_HOST=desktop-1aachgk` to avoid stale LAN
  discovery and route over Tailscale.
- Trackpad scrolling and sluggish playback remain the highest-priority user
  issues.

# Project status

Last updated: 2026-09-27

PsychBeacon is a macOS client for viewing and controlling a Windows desktop,
with physical-display mirroring or a headless virtual display. This file tracks
current work and priorities.
See [`TIMELINE.md`](TIMELINE.md) for chronological history and [`AGENTS.md`](AGENTS.md)
for architecture details.

## Current status

The end-to-end stream works over Tailscale. The host and Mac client support
multiple independent displays, with each stream configured for 30 fps. The
user reports the physical Windows display no longer blinks after reloading the
host with the capture-recovery fix. The LocalSystem service now starts its
worker in the active console session. The user reports that clicking the Mac
client's Disconnect control crashes the app, and the session currently shows
the extended desktop instead of mirroring the physical display. Video still
feels sluggish and trackpad scrolling still does not work.

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
- Mac disconnect has a floating control and window-close handling, but the user
  reports that clicking Disconnect crashes the client. Fix and verify cleanup,
  return to the picker, and reconnect before routine use.
- Windows now has an Automatic LocalSystem service supervisor. It launches a
  SYSTEM worker in the active console session, restarts it after a session
  change or unexpected exit, and keeps DXGI capture out of Session 0. The
  installer scopes inbound host ports to Tailscale IPv4 addresses and migrates
  the prior logon task. The service is installed and running on the host PC;
  the log confirms its worker started in console session 1 and bound UDP
  43701. Mac connectivity, lock-screen capture, sign-out/sign-in transitions,
  and reboot recovery still need live verification. This design serves only
  the active console session, not multiple signed-in desktops simultaneously.
  Tailscale scoping is not client authentication; tailnet ACLs must restrict
  trusted peers.
- Both requested display streams are configured for 30 fps each. Actual frame
  delivery has not been measured independently per display.

## Main development priorities

1. **Disconnect crash and session lifecycle.** The user reports that clicking
   the floating Disconnect control quits the Mac client unexpectedly. Find and
   fix the crash, ensure all receivers and sidecars stop, return to the picker,
   and verify reconnect works without quitting the app.
2. **Mirror a physical display by default.** When the host has an active
   physical display, capture and mirror that desktop so the Mac shows the same
   content as the host screen; do not create an extended VDD by default. Detect
   active physical outputs and distinguish them from Parsec VDD outputs.
3. **Headless fallback.** If no physical display is connected/active, create a
   Parsec virtual display and stream it. Keep display selection/mode explicit
   if the host has multiple eligible physical outputs. Preserve the selected
   display's resolution and refresh/FPS behavior where the capture and encoder
   support it; keep the current 30 fps target until delivery is measured.
4. **Video performance and refresh rate.** Playback feels sluggish. Measure
   capture, GPU readback, NVENC, UDP delivery, decode, and render pacing. Fix
   the bottleneck before raising the current 30 fps target; then evaluate
   60/120 fps and two-stream load.
5. **Trackpad input.** Diagnose why scroll events fail end to end, from macOS
   gesture capture through the WebSocket and Windows wheel injection. Confirm
   pinch-to-zoom in a real Windows application.
6. **Multi-display reliability.** Verify independent video and input on each
   display, reconnect behavior, and cleanup of virtual displays after normal
   disconnects and capture failures. Longer stability and per-display input
   still need confirmation.
7. **Transport resilience.** Video is raw H.264 over UDP without packet
   sequencing or retransmission. Add loss detection and a recovery strategy so
   one lost packet does not leave decoding damaged until reconnect.
8. **Clipboard scope.** Text sync works in both directions. Rich content such
   as images and files, plus clipboard history, remains future work.
9. **Security and product operation.** Add explicit session authentication
   before exposing the host beyond a restricted private tailnet; package the
   Mac picker as a normal app; then improve setup, configuration, host status,
   and diagnostics for routine use. Reboot, lock-screen, and account-switch
   behavior for the Windows service still need live verification.

## Current run notes

- Both stream targets are configured for 30 fps; this is a cap/target, not a
  measured guarantee of delivered frames.
- For this setup, use `PSYBEACON_TARGET_HOST=desktop-1aachgk` to avoid stale LAN
  discovery and route over Tailscale.
- Trackpad scrolling and sluggish playback remain the highest-priority user
  issues.

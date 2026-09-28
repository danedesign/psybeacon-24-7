# Windows app

PsychBeacon now has one Windows window for both roles: **host this PC** and
**connect to another PC**. The console host remains available for diagnostics
and for the existing Mac client.

## Build and open

From `windows-host` in PowerShell:

```powershell
cargo build --bins
.\target\debug\psybeacon.exe
```

Run the app as Administrator to host. Install the Parsec virtual display driver
and an FFmpeg build with NVENC support first; see [vdd-setup.md](docs/vdd-setup.md)
and [HOST_PC_SETUP.md](../HOST_PC_SETUP.md). The app starts
`psybeacon-host.exe` from the same directory when **Start hosting** is clicked.
It sends a graceful shutdown signal when hosting is stopped or the window
closes, so virtual displays and FFmpeg encoder processes can be cleaned up.

To connect, select a host discovered under **Nearby PCs**, or enter the
host's reachable LAN or Tailscale IPv4 address or hostname. LAN discovery uses
UDP broadcast; a host on another subnet or on Tailscale alone needs a direct
address. The Windows viewer requires `ffmpeg` on PATH. It requests one
display on UDP 43701, receives H.264 on UDP 43702, and sends keyboard/mouse
input over the host's WebSocket sidecar port. **Disconnect** sends a stop
request to the host and closes the local decoder. Allow inbound UDP 43702 on
the viewing PC if Windows Firewall blocks video.

The existing `cargo run` command still runs the console host. To launch the
window after building both binaries, use the command above.

## Current limits

- Windows viewer supports one display. The host and Mac client can negotiate
  more, but this UI has not added separate Windows viewer windows yet.
- Windows video is currently decoded by FFmpeg into JPEG frames for the app
  window. This is functional groundwork, not the planned low latency,
  high refresh Direct3D renderer. It has not yet been tested against two
  physical Windows machines.
- Session setup uses the existing unauthenticated PsychBeacon protocol. Use
  only on a trusted LAN or private Tailscale network; do not expose the
  control and input ports to the public internet.
- Both host and viewer depend on the existing virtual display and capture
  pipeline. If Desktop Duplication reports `ACCESS_LOST`, the GUI cannot
  repair that driver or session issue; the Activity tab shows the host log.

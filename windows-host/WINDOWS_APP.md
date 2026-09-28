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

Run the app as Administrator to change hosting state. Install an FFmpeg build
with NVENC support first; the Parsec virtual display driver is also needed for
headless hosting. See [vdd-setup.md](docs/vdd-setup.md) and
[HOST_PC_SETUP.md](../HOST_PC_SETUP.md). If the `PsychBeaconHost` Windows
service is installed, the app controls that service; it keeps hosting when
the window closes. Without the service, **Start hosting** launches
`psybeacon-host.exe` from the same directory and closing the window asks that
child process to stop gracefully. The service installer still requires the
full source checkout; the release ZIP is not an installer.

To connect, select a host discovered under **Nearby PCs**, or enter the
host's reachable LAN or Tailscale IPv4 address or hostname. LAN discovery uses
UDP broadcast; the installed service accepts Tailscale clients only, so use a
direct Tailscale address for it. The Windows viewer requires `ffmpeg` on PATH
or in the configured `%ProgramData%\PsychBeacon\ffmpeg-path.txt` directory. It requests one
display on UDP 43701, receives H.264 on UDP 43702, and sends keyboard/mouse
input over the host's WebSocket sidecar port. **Disconnect** closes the
sidecar and local decoder; the host cleans up when the sidecar closes. Allow inbound UDP 43702 on
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
- The host mirrors the primary physical display, or creates a virtual display
  when headless. If Desktop Duplication reports `ACCESS_LOST`, check the
  Activity tab for a standalone host or
  `%ProgramData%\PsychBeacon\logs\host.log` for the installed service.

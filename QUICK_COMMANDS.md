# Quick commands

## Start the Windows host

## One-time setup

Close any currently running host started in a PowerShell window with **Ctrl+C**
once. Then open **Administrator PowerShell** at the repository root and run:

```powershell
& ".\windows-host\scripts\install-host-task.ps1"
```

This installs and starts a hidden Scheduled Task for the signed-in Windows
user. It starts at each sign-in and runs in that user's interactive desktop
session, which DXGI capture needs. The first run checks elevation, Cargo,
FFmpeg/NVENC, the Parsec VDD driver, and the required ports. Logs go to
`%LOCALAPPDATA%\PsychBeacon\logs\host.log`.

The host should now stay available while you use that Windows account. It is a
scheduled task, not a Windows service; it needs the user to be signed in.

To stop the listener and clean up the active display:

```powershell
& ".\windows-host\scripts\stop-host.ps1"
```

To remove the automatic startup task:

```powershell
& ".\windows-host\scripts\uninstall-host-task.ps1"
```

Avoid force-killing `psybeacon-host.exe`; use the graceful stop command.

## Start the Mac client

In Mac Terminal:

```sh
cd ~/psybeacon-24-7/mac-client
PSYBEACON_TARGET_HOST=desktop-1aachgk swift run
```

This opens the computer picker. The target setting preselects that Tailscale
peer; it no longer bypasses the picker. Run `swift run` without the setting to
choose from every responding Windows host on your tailnet. To rebuild only:

```sh
cd ~/psybeacon-24-7/mac-client
swift build
```

Use the floating **Disconnect** control or close any stream window to end the
whole session and return to the computer picker. Quit the client with **⌘Q**.

## Build checks

Windows PowerShell, from `windows-host`:

```powershell
cargo check
```

Mac Terminal, from `mac-client`:

```sh
swift build
```

To run the Windows checks without starting the listener, use this from an
Administrator PowerShell in `windows-host`:

```powershell
cargo run -- --preflight
```

## Check Windows listeners

```powershell
Get-NetUDPEndpoint | Where-Object LocalPort -in 43701,43702 | Select-Object LocalAddress,LocalPort,OwningProcess
Get-NetTCPConnection -State Listen | Where-Object LocalPort -in 43703,43704,43705,43706 | Select-Object LocalAddress,LocalPort,OwningProcess
```

Ports: UDP `43701` discovery/control; UDP `43702` video for display 0; TCP
`43703` input sidecar for display 0. Additional displays use the next ports.

## Current troubleshooting notes

- The single-display stream and basic input work over Tailscale.
- The host now ends an unclaimed stream if the client never opens the sidecar
  within 15 seconds, or when the sidecar disconnects.
- Start the host before launching the Mac client.
- If the physical Windows display blinks, stop the host cleanly with **Ctrl+C**
  and see `PROJECT_STATUS.md` before reconnecting.
- Current implementation/status: [`PROJECT_STATUS.md`](PROJECT_STATUS.md).
- Historical troubleshooting: [`TIMELINE.md`](TIMELINE.md).

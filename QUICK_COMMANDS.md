# Quick commands

## Start the Windows host

## One-time setup

Open **Administrator PowerShell** at the repository root and run:

```powershell
& ".\windows-host\scripts\install-host-service.ps1"
```

This builds and installs the host as an **Automatic LocalSystem Windows
service**, then starts it. The service supervises a worker in the active
console session, so the host can remain available at the Windows sign-in/lock
screen and when a different account is signed in. It supports the active
console session only; it does not serve multiple signed-in Windows desktops
at the same time. The installer checks elevation, Cargo, FFmpeg/NVENC, the
Parsec VDD driver, and required ports. Logs go to
`%ProgramData%\PsychBeacon\logs\host.log`.

Inbound discovery and sidecar firewall rules are restricted to Tailscale IPv4
addresses (`100.64.0.0/10`). This is network scoping, not user authentication;
keep the tailnet ACL limited to trusted devices.

The service starts at boot and does not require an account to stay signed in.
The active display/lock-screen behavior still needs live verification on this
PC after installation.

To stop the listener and clean up the active display:

```powershell
& ".\windows-host\scripts\stop-host.ps1"
```

To remove the service and installed host files:

```powershell
& ".\windows-host\scripts\uninstall-host-service.ps1"
```

Check service state and recent logs with:

```powershell
Get-Service PsychBeaconHost
Get-Content "$env:ProgramData\PsychBeacon\logs\host.log" -Tail 80
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

# Quick commands

## Start the Windows host

Open **Administrator PowerShell** and run:

```powershell
& "C:\Users\headradio\Documents\Codex\2026-09-26\ins\work\psybeacon-24-7\windows-host\tools\run-host-admin.ps1"
```

Leave that window open. The startup message should say it is listening on UDP
port `43701`.

Stop it with **Ctrl+C once** in that same window. This lets the host stop FFmpeg
and clean up the virtual display. Avoid force-killing `psybeacon-host.exe`.

## Start the Mac client

In Mac Terminal:

```sh
cd ~/psybeacon-24-7/mac-client
PSYBEACON_TARGET_HOST=desktop-1aachgk swift run
```

The target setting skips LAN discovery, which can find the stale host at
`192.168.1.105`. To rebuild only:

```sh
cd ~/psybeacon-24-7/mac-client
swift build
```

Quit the client with **⌘Q**.

## Build checks

Windows PowerShell, from `windows-host`:

```powershell
cargo check
```

Mac Terminal, from `mac-client`:

```sh
swift build
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

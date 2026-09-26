# Host PC setup checklist

For setting up a **fresh Windows PC** (on the same LAN/Tailscale mesh as the
Mac client) to run `windows-host`, replacing the previous dev machine as the
host role. Written so an agent (Codex or otherwise) with an elevated
PowerShell can execute it top to bottom with minimal judgment calls — each
step says what success looks like and what to do if it doesn't.

**Footprint note**: everything here except the VDD driver (step 4) is
lightweight and cleanly removable later (`rustup self uninstall`, delete the
FFmpeg folder, delete the repo clone). The VDD driver is a real signed
Windows display driver — it has to install into this PC's actual driver
store to talk to its GPU, so it can't be sandboxed or virtualized, but it
uninstalls cleanly via Device Manager or `pnputil` when you're done.

## 0. Prerequisites check

- [ ] Windows 10 21H2+ or Windows 11.
- [ ] A discrete NVIDIA GPU (NVENC support required for module 3). Verify:
  ```powershell
  nvidia-smi
  ```
  If this fails, this PC can't do real capture/encode — discovery and
  protocol-level testing would still work, but not the actual video
  pipeline. Stop and confirm with the user before continuing on
  GPU-less hardware.
- [ ] Elevated (Administrator) PowerShell — the VDD driver install and
  `psybeacon-host` itself both need this.
- [ ] This PC's LAN IP and/or Tailscale IP, for pointing the Mac client at
  it later:
  ```powershell
  ipconfig | Select-String "IPv4"
  tailscale status  # if already installed/logged in; otherwise see step 6
  ```

## 1. Install Rust

```powershell
winget install --id Rustlang.Rustup -e
```
Restart the shell afterward so `cargo`/`rustc` are on `PATH`. Verify:
```powershell
cargo --version
```

## 2. Clone the repo

```powershell
git clone https://github.com/danedesign/psybeacon-24-7.git
cd psybeacon-24-7\windows-host
```

## 3. Build before touching drivers

Catches toolchain/environment problems early, independent of the driver:
```powershell
cargo check
```
Expected: `Finished` with no errors (a couple of pre-existing `dead_code`
warnings are fine and unrelated).

## 4. Install the Parsec VDD driver (admin required)

Full detail in [`windows-host/docs/vdd-setup.md`](windows-host/docs/vdd-setup.md).
Short version — download, then silent install:
```powershell
Invoke-WebRequest -Uri "https://builds.parsec.app/vdd/parsec-vdd-0.45.0.0.exe" -OutFile "parsec-vdd-0.45.0.0.exe"
.\parsec-vdd-0.45.0.0.exe /S
```
No confirmation dialog on success (silent install). If `nefconw`/manual
driver-node steps are needed instead (silent install failed), see the doc's
step 2 alternative.

## 5. Install FFmpeg with NVENC support

**Known trap, hit before on this project**: some FFmpeg builds (e.g. Gyan's)
are compiled against a newer NVENC SDK than an older NVIDIA driver exposes,
and fail outright with no useful error. Prefer a BtbN build, which worked
where Gyan's didn't:
```powershell
# Download a recent BtbN "full" build (win64, gpl or lgpl) from:
# https://github.com/BtbN/FFmpeg-Builds/releases
# Unzip it, add its bin\ folder to PATH, then verify:
ffmpeg -h encoder=h264_nvenc
```
Expected: prints `h264_nvenc` option list (presets, `-tune`, etc.), not an
error. If this errors, try an older BtbN release before assuming the GPU
driver itself needs updating.

## 6. Tailscale (if not already installed, and LAN alone isn't reachable)

```powershell
winget install --id Tailscale.Tailscale -e
```
Then `tailscale up` and log in. Confirms this PC's `100.x.x.x` mesh IP for
the Mac client's `PSYBEACON_TARGET_HOST` fallback if it's not on the same
physical LAN subnet as the MacBook.

## 7. Windows Firewall — allow inbound discovery + stream ports

```powershell
New-NetFirewallRule -DisplayName "PsychBeacon Discovery" -Direction Inbound -Protocol UDP -LocalPort 43701 -Action Allow
New-NetFirewallRule -DisplayName "PsychBeacon Streams" -Direction Inbound -Protocol UDP -LocalPort 43702-43706 -Action Allow
New-NetFirewallRule -DisplayName "PsychBeacon Sidecar" -Direction Inbound -Protocol TCP -LocalPort 43703-43707 -Action Allow
```
(Port ranges cover up to `MAX_DISPLAY_COUNT` = 4 displays' worth of stream
and sidecar ports with headroom — see `windows-host/src/main.rs`.)

## 8. Read-only sanity check (no display added yet)

```powershell
cd psybeacon-24-7\windows-host
$env:RUST_LOG="info"; cargo run -- --diagnose-capture
```
Expected right after a fresh driver install: `no output found matching
"Parsec Virtual Display Adapter"` — that's correct, nothing's been added
yet. This just confirms the binary runs and can enumerate DXGI outputs
without crashing.

## 9. Run the real launcher, elevated

```powershell
cargo run
```
Expected log lines, in order:
```
VDD: added virtual display index 0
VDD: virtual display 0 is up and pinging (driver version: Ok(45))
Listening for discovery broadcasts on UDP :43701
```
If instead you see "entity not found" or `ERROR_ACCESS_DENIED` warnings,
either the shell isn't actually elevated or the driver install in step 4
didn't take — re-check both before continuing.

## 10. Test from the Mac client

From the MacBook (Codex or otherwise), pointed at this PC's LAN or
Tailscale IP:
```bash
cd psybeacon-24-7/mac-client
PSYBEACON_TARGET_HOST=<this-pc-hostname-or-substring> swift run
```
Watch this PC's `cargo run` terminal for `Multi-stream: request from ...`
and per-display `is up` log lines, and the Mac's terminal for
`DisplayNegotiator`/`DisplayWindowController` lines. Report both logs back
verbatim rather than paraphrasing "it worked"/"it didn't" — the specifics
are what let the next step be diagnosis instead of another guess.

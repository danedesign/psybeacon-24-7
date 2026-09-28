# Installing the Parsec VDD driver

`windows-host` talks to the driver over IOCTLs (`src/vdd.rs`) but does not
install it — the driver binary (`mm.inf`/`mm.dll`/`mm.cat`) is proprietary,
built and signed by Parsec, and hosted on Parsec's own servers, not in the
[nomi-san/parsec-vdd](https://github.com/nomi-san/parsec-vdd) wrapper repo
this module is ported from. Install it yourself before running the
launcher for real:

## 1. Download

Pick one from upstream's compatibility table (newest generally best unless
you hit the "may not work on some Windows" note):

- [parsec-vdd-0.45.0.0.exe](https://builds.parsec.app/vdd/parsec-vdd-0.45.0.0.exe) — Windows 10 21H2+, IddCx 1.5
- [parsec-vdd-0.41.0.0.exe](https://builds.parsec.app/vdd/parsec-vdd-0.41.0.0.exe) — Windows 10 19H2+, IddCx 1.4, stable
- [parsec-vdd-0.38.0.0.exe](https://builds.parsec.app/vdd/parsec-vdd-0.38.0.0.exe) — obsolete, may crash randomly

## 2. Install (admin required)

Silent install:

```
.\parsec-vdd-0.45.0.0.exe /S
```

Or unzip it (7z) to get `nefconw.exe` + `driver\mm.inf` and drive it
manually:

```
start /wait .\nefconw.exe --remove-device-node --hardware-id Root\Parsec\VDA --class-guid "4D36E968-E325-11CE-BFC1-08002BE10318"
start /wait .\nefconw.exe --create-device-node --class-name Display --class-guid "4D36E968-E325-11CE-BFC1-08002BE10318" --hardware-id Root\Parsec\VDA
start /wait .\nefconw.exe --install-driver --inf-path ".\driver\mm.inf"
```

## 3. Run `psybeacon-host` elevated

Two separate things in this codebase need admin rights, both of which fail
*gracefully* (logged warning, launcher keeps running) if you skip this —
that's how the behavior below was verified, on this dev machine, without
either:

- Writing the custom-resolution registry presets (`HKLM\SOFTWARE\Parsec\vdd\<slot>`) — `ERROR_ACCESS_DENIED` without elevation.
- Opening the VDD device handle if the driver isn't installed yet — reported as "entity not found".

## Verify

```
cargo run
```

With the driver installed and the process elevated, you should see
`VDD: virtual display 0 is up and pinging (driver version: ...)` instead of
the fallback warnings, and a new display should appear in Windows' Display
Settings.

The normal admin launcher runs `cargo run -- --preflight` before starting the
listener. This checks that the VDD device responds, FFmpeg includes NVENC, and
the discovery and sidecar ports are available. To run that check manually
without starting the listener:

```
cargo run -- --preflight
```

**Confirmed working 2026-09-22** against driver v0.45.0.0 (`driver version: Ok(45)`
in the log) — full sequence: registry preset write, device open, add-display,
heartbeat, version query. If you hit `ERROR_INVALID_PARAMETER` from
`CreateFileW` specifically, that's a bug that existed in this repo before
that date (wrong access-rights flags) rather than anything you did — make
sure you're on a version of `vdd.rs` at or after that fix.

## Alternative implementation

[rohitsangwan01/parsec-vdd-rust](https://github.com/rohitsangwan01/parsec-vdd-rust)
is an existing Rust port of the same wrapper, listed in upstream's
"Projects" section. `src/vdd.rs` here is a from-scratch port made directly
from `core/parsec-vdd.h` + `docs/PARSEC_VDD_RE.md` for this project's own
lifecycle needs (the `VddSession` RAII wrapper, the corrected heartbeat
interval); worth a look if you'd rather depend on a maintained crate than
this vendored module.

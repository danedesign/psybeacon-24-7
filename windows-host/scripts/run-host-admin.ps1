$ErrorActionPreference = 'Stop'
$hostDir = Split-Path $PSScriptRoot -Parent
$tools = Join-Path $hostDir 'tools'
$ffmpegDir = Join-Path $tools 'ffmpeg-7.1.5\ffmpeg-n7.1.5-12-g1fdbca85aa-win64-gpl-7.1\bin'

$principal = [Security.Principal.WindowsPrincipal]::new([Security.Principal.WindowsIdentity]::GetCurrent())
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    throw 'PsychBeacon host requires Administrator privileges. Open PowerShell as Administrator and run this script again.'
}

$cargoDir = Join-Path $env:USERPROFILE '.cargo\bin'
if (-not (Test-Path (Join-Path $cargoDir 'cargo.exe'))) {
    throw "Cargo was not found at $cargoDir. Install Rust with rustup, then reopen PowerShell."
}
if (-not (Test-Path (Join-Path $ffmpegDir 'ffmpeg.exe'))) {
    throw "Bundled FFmpeg was not found at $ffmpegDir. Restore the local windows-host/tools/ffmpeg-7.1.5 folder."
}

$env:Path = $cargoDir + ';' + $ffmpegDir + ';' + $env:Path
if (-not $env:RUST_LOG) { $env:RUST_LOG = 'info' }
Set-Location $hostDir

cargo run -- --preflight
if ($LASTEXITCODE -ne 0) {
    throw 'Preflight failed. Resolve the reported dependency or port issue before starting the host.'
}

cargo run

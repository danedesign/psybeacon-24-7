$ErrorActionPreference = 'Stop'
$hostDir = Split-Path $PSScriptRoot -Parent
$cargoDir = Join-Path $env:USERPROFILE '.cargo\bin'
$hostExe = Join-Path $hostDir 'target\debug\psybeacon-host.exe'

if (-not (Test-Path -LiteralPath $hostExe)) {
    throw "Host executable not found at $hostExe. Install/start the host task first."
}
if (Test-Path -LiteralPath (Join-Path $cargoDir 'cargo.exe')) {
    $env:Path = $cargoDir + ';' + $env:Path
}
& $hostExe --stop
if ($LASTEXITCODE -ne 0) {
    throw 'The host did not acknowledge the stop request. Check that it is running the updated version.'
}

$taskName = 'PsychBeacon Host'
$deadline = (Get-Date).AddSeconds(20)
do {
    Start-Sleep -Milliseconds 500
    $task = Get-ScheduledTask -TaskName $taskName -ErrorAction SilentlyContinue
} while ($task -and $task.State -eq 'Running' -and (Get-Date) -lt $deadline)
Write-Host 'PsychBeacon host stopped gracefully.'

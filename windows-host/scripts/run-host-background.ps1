$ErrorActionPreference = 'Stop'
$hostDir = Split-Path $PSScriptRoot -Parent
$logDir = Join-Path $env:LOCALAPPDATA 'PsychBeacon\logs'
$logFile = Join-Path $logDir 'host.log'
New-Item -ItemType Directory -Path $logDir -Force | Out-Null

"`n===== Host started $(Get-Date -Format o) =====" | Add-Content -LiteralPath $logFile
& (Join-Path $PSScriptRoot 'run-host-admin.ps1') *>> $logFile
$exitCode = $LASTEXITCODE
"===== Host exited $(Get-Date -Format o), code $exitCode =====" | Add-Content -LiteralPath $logFile
exit $exitCode

$ErrorActionPreference = 'Stop'
$taskName = 'PsychBeacon Host'
$task = Get-ScheduledTask -TaskName $taskName -ErrorAction SilentlyContinue
if (-not $task) {
    Write-Host "Task '$taskName' is not installed."
    exit 0
}

if ($task.State -eq 'Running') {
    & (Join-Path $PSScriptRoot 'stop-host.ps1')
}
Unregister-ScheduledTask -TaskName $taskName -Confirm:$false
Write-Host "Removed '$taskName'."

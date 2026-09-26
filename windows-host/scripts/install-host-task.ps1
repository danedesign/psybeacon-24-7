$ErrorActionPreference = 'Stop'
$principal = [Security.Principal.WindowsPrincipal]::new([Security.Principal.WindowsIdentity]::GetCurrent())
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    throw 'Run this once from Administrator PowerShell.'
}

$taskName = 'PsychBeacon Host'
$backgroundScript = Join-Path $PSScriptRoot 'run-host-background.ps1'
$shell = Join-Path $PSHOME 'pwsh.exe'
if (-not (Test-Path -LiteralPath $shell)) {
    $shell = Join-Path $env:WINDIR 'System32\WindowsPowerShell\v1.0\powershell.exe'
}
if (-not (Test-Path -LiteralPath $shell)) {
    throw 'Could not locate pwsh.exe or Windows PowerShell to run the host task.'
}
$arguments = "-NoProfile -ExecutionPolicy Bypass -WindowStyle Hidden -File `"$backgroundScript`""
$user = [Security.Principal.WindowsIdentity]::GetCurrent().Name

# Interactive logon keeps DXGI desktop capture in the signed-in user's desktop
# session. A Session 0 Windows service cannot capture that desktop reliably.
$action = New-ScheduledTaskAction -Execute $shell -Argument $arguments -WorkingDirectory (Split-Path $PSScriptRoot -Parent)
$trigger = New-ScheduledTaskTrigger -AtLogOn -User $user
$taskPrincipal = New-ScheduledTaskPrincipal -UserId $user -LogonType Interactive -RunLevel Highest
$settings = New-ScheduledTaskSettingsSet `
    -MultipleInstances IgnoreNew `
    -ExecutionTimeLimit ([TimeSpan]::Zero) `
    -RestartCount 3 `
    -RestartInterval (New-TimeSpan -Minutes 1) `
    -StartWhenAvailable

Register-ScheduledTask -TaskName $taskName -Action $action -Trigger $trigger -Principal $taskPrincipal -Settings $settings -Description 'Runs the PsychBeacon Windows host in the signed-in desktop session.' -Force | Out-Null
Start-ScheduledTask -TaskName $taskName
Write-Host "Installed and started '$taskName'. It will start hidden when $user signs in."
Write-Host "Host log: $(Join-Path $env:LOCALAPPDATA 'PsychBeacon\logs\host.log')"
Write-Host 'Stop gracefully with: .\windows-host\scripts\stop-host.ps1'

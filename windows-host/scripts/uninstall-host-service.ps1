$ErrorActionPreference = 'Stop'
$serviceName = 'PsychBeaconHost'
$trayTaskName = 'PsychBeacon Host Tray'
$firewallNames = @('PsychBeacon Host Tailscale UDP', 'PsychBeacon Host Tailscale Sidecar TCP', 'PsychBeacon Client Tailscale Video UDP')
$installDir = Join-Path $env:ProgramFiles 'PsychBeaconHost'
$installedTray = Join-Path $installDir 'host-tray.ps1'

$principal = [Security.Principal.WindowsPrincipal]::new([Security.Principal.WindowsIdentity]::GetCurrent())
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    throw 'Run this from Administrator PowerShell.'
}

$trayTask = Get-ScheduledTask -TaskName $trayTaskName -ErrorAction SilentlyContinue
if ($trayTask) {
    if ($trayTask.State -eq 'Running') { Stop-ScheduledTask -TaskName $trayTaskName -ErrorAction SilentlyContinue }
    Unregister-ScheduledTask -TaskName $trayTaskName -Confirm:$false
}
Get-CimInstance Win32_Process -Filter "Name = 'powershell.exe'" -ErrorAction SilentlyContinue |
    Where-Object { $_.CommandLine -and $_.CommandLine.Contains($installedTray) } |
    ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }

$service = Get-Service -Name $serviceName -ErrorAction SilentlyContinue
if ($service) {
    if ($service.Status -ne 'Stopped') {
        Stop-Service -Name $serviceName
        (Get-Service -Name $serviceName).WaitForStatus('Stopped', [TimeSpan]::FromSeconds(45))
    }
    sc.exe delete $serviceName | Out-Null
}
Get-NetFirewallRule -DisplayName $firewallNames -ErrorAction SilentlyContinue | Remove-NetFirewallRule

if (Test-Path -LiteralPath $installDir) {
    Remove-Item -LiteralPath $installDir -Recurse -Force
}
Write-Host "Removed the PsychBeacon service, firewall rules, and installed program files. Logs remain under $env:ProgramData\PsychBeacon\logs."

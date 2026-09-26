$ErrorActionPreference = 'Stop'
$serviceName = 'PsychBeaconHost'
$firewallNames = @('PsychBeacon Host Tailscale UDP', 'PsychBeacon Host Tailscale Sidecar TCP')

$principal = [Security.Principal.WindowsPrincipal]::new([Security.Principal.WindowsIdentity]::GetCurrent())
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    throw 'Run this from Administrator PowerShell.'
}

$service = Get-Service -Name $serviceName -ErrorAction SilentlyContinue
if ($service) {
    if ($service.Status -ne 'Stopped') {
        Stop-Service -Name $serviceName
        (Get-Service -Name $serviceName).WaitForStatus('Stopped', [TimeSpan]::FromSeconds(45))
    }
    sc.exe delete $serviceName | Out-Null
}
Get-NetFirewallRule -DisplayName $firewallNames -ErrorAction SilentlyContinue | Remove-NetFirewallRule

$installDir = Join-Path $env:ProgramFiles 'PsychBeaconHost'
if (Test-Path -LiteralPath $installDir) {
    Remove-Item -LiteralPath $installDir -Recurse -Force
}
Write-Host "Removed the PsychBeacon service, firewall rules, and installed program files. Logs remain under $env:ProgramData\PsychBeacon\logs."

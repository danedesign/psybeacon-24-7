$ErrorActionPreference = 'Stop'
$serviceName = 'PsychBeaconHost'
$service = Get-Service -Name $serviceName -ErrorAction SilentlyContinue
if (-not $service) {
    throw "The '$serviceName' service is not installed."
}
if ($service.Status -ne 'Stopped') {
    Stop-Service -Name $serviceName
    (Get-Service -Name $serviceName).WaitForStatus('Stopped', [TimeSpan]::FromSeconds(45))
}
Write-Host 'PsychBeacon Host stopped gracefully; active virtual displays have been released.'

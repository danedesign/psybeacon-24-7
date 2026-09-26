$ErrorActionPreference = 'Stop'
$hostDir = Split-Path $PSScriptRoot -Parent
$repoRoot = Split-Path $hostDir -Parent
$cargoDir = Join-Path $env:USERPROFILE '.cargo\bin'
$ffmpegSource = Join-Path $hostDir 'tools\ffmpeg-7.1.5\ffmpeg-n7.1.5-12-g1fdbca85aa-win64-gpl-7.1\bin'
$installDir = Join-Path $env:ProgramFiles 'PsychBeaconHost'
$installedFfmpeg = Join-Path $installDir 'ffmpeg'
$installedExe = Join-Path $installDir 'psybeacon-host.exe'
$serviceName = 'PsychBeaconHost'
$taskName = 'PsychBeacon Host'
$firewallUdp = 'PsychBeacon Host Tailscale UDP'
$firewallTcp = 'PsychBeacon Host Tailscale Sidecar TCP'
$remoteTailscale = '100.64.0.0/10'
$logDir = Join-Path $env:ProgramData 'PsychBeacon\logs'

$principal = [Security.Principal.WindowsPrincipal]::new([Security.Principal.WindowsIdentity]::GetCurrent())
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    throw 'Run this once from Administrator PowerShell.'
}
if (-not (Test-Path -LiteralPath (Join-Path $cargoDir 'cargo.exe'))) {
    throw "Cargo was not found at $cargoDir. Install Rust with rustup, then reopen PowerShell."
}
if (-not (Test-Path -LiteralPath (Join-Path $ffmpegSource 'ffmpeg.exe'))) {
    throw "The local FFmpeg bundle was not found at $ffmpegSource. Restore windows-host/tools/ffmpeg-7.1.5 first."
}

$env:Path = $cargoDir + ';' + $ffmpegSource + ';' + $env:Path
$env:PSYBEACON_FFMPEG_DIR = $ffmpegSource
Set-Location $repoRoot
Write-Host 'Building the Windows host in release mode...'
cargo build --release --manifest-path (Join-Path $hostDir 'Cargo.toml')
if ($LASTEXITCODE -ne 0) { throw 'The release build failed.' }

$releaseExe = Join-Path $hostDir 'target\release\psybeacon-host.exe'

# Stop and remove the old service before replacing its executable. Windows
# keeps the service image locked for the lifetime of the service process.
$existingService = Get-Service -Name $serviceName -ErrorAction SilentlyContinue
if ($existingService) {
    if ($existingService.Status -ne 'Stopped') {
        Write-Host 'Stopping the existing PsychBeacon service before updating its executable...'
        Stop-Service -Name $serviceName
        (Get-Service -Name $serviceName).WaitForStatus('Stopped', [TimeSpan]::FromSeconds(45))
    }
    sc.exe delete $serviceName | Out-Null
    Start-Sleep -Seconds 2
}

# Stop any older interactive launch cleanly before copying the new binary.
$listener = Get-NetUDPEndpoint -LocalPort 43701 -ErrorAction SilentlyContinue
if ($listener) {
    Write-Host 'Stopping the currently running host cleanly before service installation...'
    & $releaseExe --stop
    if ($LASTEXITCODE -ne 0) {
        throw 'The current listener did not acknowledge shutdown. Stop its PowerShell host with Ctrl+C, then run this installer again.'
    }
    $deadline = (Get-Date).AddSeconds(30)
    do {
        Start-Sleep -Milliseconds 500
        $listener = Get-NetUDPEndpoint -LocalPort 43701 -ErrorAction SilentlyContinue
    } while ($listener -and (Get-Date) -lt $deadline)
    if ($listener) { throw 'The listener has not released UDP 43701; check the host log before continuing.' }
}

# Remove the previous logon-task setup if it was installed.
$oldTask = Get-ScheduledTask -TaskName $taskName -ErrorAction SilentlyContinue
if ($oldTask) {
    if ($oldTask.State -eq 'Running') { Stop-ScheduledTask -TaskName $taskName -ErrorAction SilentlyContinue }
    Unregister-ScheduledTask -TaskName $taskName -Confirm:$false
}

& $releaseExe --preflight
if ($LASTEXITCODE -ne 0) {
    throw 'Host preflight failed. Resolve the reported driver, FFmpeg, or port issue before installing the service.'
}

New-Item -ItemType Directory -Path $installDir -Force | Out-Null
New-Item -ItemType Directory -Path $installedFfmpeg -Force | Out-Null
Copy-Item -LiteralPath $releaseExe -Destination $installedExe -Force
Copy-Item -Path (Join-Path $ffmpegSource '*') -Destination $installedFfmpeg -Recurse -Force
New-Item -ItemType Directory -Path $logDir -Force | Out-Null
$configDir = Join-Path $env:ProgramData 'PsychBeacon'
New-Item -ItemType Directory -Path $configDir -Force | Out-Null
Set-Content -LiteralPath (Join-Path $configDir 'ffmpeg-path.txt') -Value $installedFfmpeg -Encoding UTF8
$configAcl = [System.Security.AccessControl.DirectorySecurity]::new()
$configAcl.SetAccessRuleProtection($true, $false)
$configAcl.SetOwner([System.Security.Principal.NTAccount]::new('BUILTIN\Administrators'))
foreach ($entry in @(
    @{ Identity = 'NT AUTHORITY\SYSTEM'; Rights = 'FullControl' },
    @{ Identity = 'BUILTIN\Administrators'; Rights = 'FullControl' },
    @{ Identity = 'BUILTIN\Users'; Rights = 'ReadAndExecute' }
)) {
    $rule = [System.Security.AccessControl.FileSystemAccessRule]::new(
        $entry.Identity,
        $entry.Rights,
        'ContainerInherit,ObjectInherit',
        'None',
        'Allow'
    )
    $configAcl.AddAccessRule($rule)
}
Set-Acl -LiteralPath $configDir -AclObject $configAcl
$env:PSYBEACON_FFMPEG_DIR = $installedFfmpeg
$env:PSYBEACON_LOG_FILE = Join-Path $logDir 'host.log'

Get-NetFirewallRule -DisplayName $firewallUdp, $firewallTcp -ErrorAction SilentlyContinue | Remove-NetFirewallRule
New-NetFirewallRule -DisplayName $firewallUdp -Direction Inbound -Action Allow -Program $installedExe -Protocol UDP -LocalPort 43701 -RemoteAddress $remoteTailscale -Profile Any | Out-Null
New-NetFirewallRule -DisplayName $firewallTcp -Direction Inbound -Action Allow -Program $installedExe -Protocol TCP -LocalPort '43703-43706' -RemoteAddress $remoteTailscale -Profile Any | Out-Null

$binaryPath = '"{0}" --service' -f $installedExe
New-Service -Name $serviceName -DisplayName 'PsychBeacon Host' -Description 'Provides Tailscale-only access to the active Windows console desktop.' -BinaryPathName $binaryPath -StartupType Automatic | Out-Null
& sc.exe failure $serviceName reset= 86400 actions= restart/5000/restart/5000/restart/5000 | Out-Null
Start-Service -Name $serviceName
(Get-Service -Name $serviceName).WaitForStatus('Running', [TimeSpan]::FromSeconds(30))

$logPath = Join-Path $logDir 'host.log'
$workerDeadline = (Get-Date).AddSeconds(30)
$workerStarted = $false
do {
    $recentLog = @(Get-Content -LiteralPath $logPath -Tail 100 -ErrorAction SilentlyContinue)
    $serviceStartIndex = -1
    for ($index = $recentLog.Count - 1; $index -ge 0; $index--) {
        if ($recentLog[$index] -match 'PsychBeacon system service started') {
            $serviceStartIndex = $index
            break
        }
    }
    $sessionLog = @()
    if ($serviceStartIndex -ge 0 -and $serviceStartIndex + 1 -lt $recentLog.Count) {
        $sessionLog = $recentLog[($serviceStartIndex + 1)..($recentLog.Count - 1)]
    }
    if ($sessionLog -match 'Started SYSTEM desktop worker in console session') {
        $workerStarted = $true
        break
    }
    if ($sessionLog -match "Couldn't launch desktop worker in session") {
        throw "The service is running, but its desktop worker failed to launch. Recent log: $logPath`n$($sessionLog | Select-Object -Last 12 | Out-String)"
    }
    Start-Sleep -Seconds 1
} while ((Get-Date) -lt $workerDeadline)
if (-not $workerStarted) {
    throw "The service started, but no desktop worker became ready within 30 seconds. Check $logPath"
}

Write-Host 'PsychBeacon Host is installed as an Automatic LocalSystem service.'
Write-Host 'It supervises a SYSTEM worker in the active console session, including the Windows sign-in/locked desktop.'
Write-Host "Firewall access is restricted to Tailscale IPv4 peers ($remoteTailscale). Keep your tailnet ACL limited to trusted devices."
Write-Host "Log file: $logPath"

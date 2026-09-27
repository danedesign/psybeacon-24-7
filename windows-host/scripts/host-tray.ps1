Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing

$serviceName = 'PsychBeaconHost'
$logPath = Join-Path $env:ProgramData 'PsychBeacon\logs\host.log'
$menu = New-Object System.Windows.Forms.ContextMenuStrip
$statusItem = $menu.Items.Add('PsychBeacon Host: checking...')
$statusItem.Enabled = $false
$menu.Items.Add((New-Object System.Windows.Forms.ToolStripSeparator)) | Out-Null
$startItem = $menu.Items.Add('Start host')
$stopItem = $menu.Items.Add('Stop host')
$startItem.add_Click({
    try {
        Start-Process -FilePath (Join-Path $env:SystemRoot 'System32\sc.exe') -Verb RunAs -ArgumentList @('start', $serviceName) -Wait | Out-Null
        Update-MenuState
    } catch {
        [System.Windows.Forms.MessageBox]::Show("Couldn't start the host service: $($_.Exception.Message)", 'PsychBeacon Host') | Out-Null
    }
})
$stopItem.add_Click({
    try {
        Start-Process -FilePath (Join-Path $env:SystemRoot 'System32\sc.exe') -Verb RunAs -ArgumentList @('stop', $serviceName) -Wait | Out-Null
        Update-MenuState
    } catch {
        [System.Windows.Forms.MessageBox]::Show("Couldn't stop the host service: $($_.Exception.Message)", 'PsychBeacon Host') | Out-Null
    }
})
$menu.Items.Add((New-Object System.Windows.Forms.ToolStripSeparator)) | Out-Null
$logsItem = $menu.Items.Add('Open host log')
$menu.Items.Add('Quit tray icon') | Out-Null
$quitItem = $menu.Items[$menu.Items.Count - 1]

$icon = New-Object System.Windows.Forms.NotifyIcon
$icon.Icon = [System.Drawing.SystemIcons]::Application
$icon.Text = 'PsychBeacon Host'
$icon.ContextMenuStrip = $menu
$icon.Visible = $true

function Get-ConnectionState {
    if (-not (Test-Path -LiteralPath $logPath)) { return 'No session' }
    $lines = @(Get-Content -LiteralPath $logPath -Tail 500 -ErrorAction SilentlyContinue)
    $lastConnect = -1
    $lastDisconnect = -1
    for ($i = 0; $i -lt $lines.Count; $i++) {
        if ($lines[$i] -match 'Sidecar: connection from .*') { $lastConnect = $i }
        if ($lines[$i] -match 'Sidecar: client disconnected|Sidecar: connection from .* ended|Multi-stream: session .* ended|no client connected .* ending the display session') {
            $lastDisconnect = $i
        }
    }
    if ($lastConnect -gt $lastDisconnect) { return 'Connected' }
    return 'No session'
}

function Update-MenuState {
    $service = Get-Service -Name $serviceName -ErrorAction SilentlyContinue
    if (-not $service) {
        $state = 'Not installed'
        $running = $false
    } else {
        $running = $service.Status -eq [System.ServiceProcess.ServiceControllerStatus]::Running
        $state = if ($running) { 'Running' } else { [string]$service.Status }
    }
    $connection = if ($running) { Get-ConnectionState } else { 'Unavailable' }
    $statusItem.Text = "Host: $state  |  Session: $connection"
    $startItem.Enabled = $null -ne $service -and -not $running
    $stopItem.Enabled = $null -ne $service -and $running
    $icon.Text = "PsychBeacon: $state"
}

$menu.add_Opening({ Update-MenuState })
$logsItem.add_Click({
    if (Test-Path -LiteralPath $logPath) { Start-Process notepad.exe -ArgumentList @($logPath) }
    else { [System.Windows.Forms.MessageBox]::Show("The host log doesn't exist yet: $logPath", 'PsychBeacon Host') | Out-Null }
})
$quitItem.add_Click({ $icon.Visible = $false; $icon.Dispose(); $menu.Dispose(); [System.Windows.Forms.Application]::Exit() })
$icon.add_DoubleClick({ $menu.Show([System.Windows.Forms.Cursor]::Position) })

$timer = New-Object System.Windows.Forms.Timer
$timer.Interval = 5000
$timer.add_Tick({ Update-MenuState })
$timer.Start()
Update-MenuState
[System.Windows.Forms.Application]::Run()
$timer.Stop()
$timer.Dispose()
$icon.Visible = $false
$icon.Dispose()
$menu.Dispose()

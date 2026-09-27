Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing

$hookSource = @'
using System;
using System.Collections.Concurrent;
using System.Diagnostics;
using System.Runtime.InteropServices;

public sealed class PsychBeaconInputEvent {
    public string Type { get; set; }
    public double X { get; set; }
    public double Y { get; set; }
    public double DeltaX { get; set; }
    public double DeltaY { get; set; }
    public string Button { get; set; }
    public int KeyCode { get; set; }
}

public sealed class PsychBeaconInputHook : IDisposable {
    private const int WH_MOUSE_LL = 14, WH_KEYBOARD_LL = 13;
    private const int HC_ACTION = 0;
    private const int WM_MOUSEMOVE = 0x0200, WM_LBUTTONDOWN = 0x0201, WM_LBUTTONUP = 0x0202;
    private const int WM_RBUTTONDOWN = 0x0204, WM_RBUTTONUP = 0x0205;
    private const int WM_MBUTTONDOWN = 0x0207, WM_MBUTTONUP = 0x0208, WM_MOUSEWHEEL = 0x020A, WM_MOUSEHWHEEL = 0x020E;
    private const int WM_KEYDOWN = 0x0100, WM_KEYUP = 0x0101, WM_SYSKEYDOWN = 0x0104, WM_SYSKEYUP = 0x0105;
    private readonly int targetPid;
    private readonly HookProc mouseCallback, keyboardCallback;
    private readonly IntPtr mouseHook, keyboardHook;
    private long lastMoveTick;
    public readonly ConcurrentQueue<PsychBeaconInputEvent> Events = new ConcurrentQueue<PsychBeaconInputEvent>();

    public PsychBeaconInputHook(int processId) {
        targetPid = processId;
        mouseCallback = MouseProc;
        keyboardCallback = KeyboardProc;
        IntPtr module = GetModuleHandle(null);
        mouseHook = SetWindowsHookEx(WH_MOUSE_LL, mouseCallback, module, 0);
        keyboardHook = SetWindowsHookEx(WH_KEYBOARD_LL, keyboardCallback, module, 0);
        if (mouseHook == IntPtr.Zero || keyboardHook == IntPtr.Zero) {
            Dispose();
            throw new System.ComponentModel.Win32Exception(Marshal.GetLastWin32Error(), "Could not install the remote input hooks");
        }
    }

    public bool TryDequeue(out PsychBeaconInputEvent item) { return Events.TryDequeue(out item); }

    private IntPtr MouseProc(int code, IntPtr message, IntPtr dataPtr) {
        if (code == HC_ACTION) {
            int msg = message.ToInt32();
            MSLLHOOKSTRUCT data = Marshal.PtrToStructure<MSLLHOOKSTRUCT>(dataPtr);
            IntPtr target = WindowFromPoint(data.pt);
            uint pid; GetWindowThreadProcessId(target, out pid);
            if ((int)pid == targetPid) {
                if (msg == WM_MOUSEMOVE) {
                    long now = Stopwatch.GetTimestamp();
                    long prior = lastMoveTick;
                    if (now - prior < Stopwatch.Frequency / 90) return CallNextHookEx(mouseHook, code, message, dataPtr);
                    lastMoveTick = now;
                    RECT r; POINT p = data.pt;
                    if (GetClientRect(target, out r) && ScreenToClient(target, ref p)) {
                        double w = Math.Max(1, r.Right - r.Left), h = Math.Max(1, r.Bottom - r.Top);
                        Events.Enqueue(new PsychBeaconInputEvent { Type = "mouseMove", X = Math.Max(0, Math.Min(1, p.X / w)), Y = Math.Max(0, Math.Min(1, p.Y / h)) });
                    }
                } else if (msg == WM_LBUTTONDOWN || msg == WM_LBUTTONUP || msg == WM_RBUTTONDOWN || msg == WM_RBUTTONUP || msg == WM_MBUTTONDOWN || msg == WM_MBUTTONUP) {
                    string button = (msg == WM_LBUTTONDOWN || msg == WM_LBUTTONUP) ? "left" : ((msg == WM_RBUTTONDOWN || msg == WM_RBUTTONUP) ? "right" : "middle");
                    string type = (msg == WM_LBUTTONDOWN || msg == WM_RBUTTONDOWN || msg == WM_MBUTTONDOWN) ? "mouseDown" : "mouseUp";
                    Events.Enqueue(new PsychBeaconInputEvent { Type = type, Button = button });
                } else if (msg == WM_MOUSEWHEEL || msg == WM_MOUSEHWHEEL) {
                    short delta = unchecked((short)((data.mouseData >> 16) & 0xffff));
                    Events.Enqueue(new PsychBeaconInputEvent { Type = "scroll", DeltaX = msg == WM_MOUSEHWHEEL ? delta : 0, DeltaY = msg == WM_MOUSEWHEEL ? delta : 0 });
                }
            }
        }
        return CallNextHookEx(mouseHook, code, message, dataPtr);
    }

    private IntPtr KeyboardProc(int code, IntPtr message, IntPtr dataPtr) {
        if (code == HC_ACTION) {
            int msg = message.ToInt32();
            if (msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN || msg == WM_KEYUP || msg == WM_SYSKEYUP) {
                IntPtr foreground = GetForegroundWindow();
                uint pid; GetWindowThreadProcessId(foreground, out pid);
                if ((int)pid == targetPid) {
                    KBDLLHOOKSTRUCT key = Marshal.PtrToStructure<KBDLLHOOKSTRUCT>(dataPtr);
                    Events.Enqueue(new PsychBeaconInputEvent { Type = (msg == WM_KEYUP || msg == WM_SYSKEYUP) ? "keyUp" : "keyDown", KeyCode = (int)key.vkCode });
                }
            }
        }
        return CallNextHookEx(keyboardHook, code, message, dataPtr);
    }

    public void Dispose() {
        if (mouseHook != IntPtr.Zero) UnhookWindowsHookEx(mouseHook);
        if (keyboardHook != IntPtr.Zero) UnhookWindowsHookEx(keyboardHook);
    }

    private delegate IntPtr HookProc(int code, IntPtr message, IntPtr data);
    [StructLayout(LayoutKind.Sequential)] private struct POINT { public int X, Y; }
    [StructLayout(LayoutKind.Sequential)] private struct RECT { public int Left, Top, Right, Bottom; }
    [StructLayout(LayoutKind.Sequential)] private struct MSLLHOOKSTRUCT { public POINT pt; public uint mouseData, flags, time; public UIntPtr extraInfo; }
    [StructLayout(LayoutKind.Sequential)] private struct KBDLLHOOKSTRUCT { public uint vkCode, scanCode, flags, time; public UIntPtr extraInfo; }
    [DllImport("user32.dll", SetLastError=true)] private static extern IntPtr SetWindowsHookEx(int id, HookProc callback, IntPtr module, uint threadId);
    [DllImport("user32.dll", SetLastError=true)] private static extern bool UnhookWindowsHookEx(IntPtr hook);
    [DllImport("user32.dll")] private static extern IntPtr CallNextHookEx(IntPtr hook, int code, IntPtr message, IntPtr data);
    [DllImport("user32.dll")] private static extern IntPtr WindowFromPoint(POINT point);
    [DllImport("user32.dll")] private static extern bool GetClientRect(IntPtr hwnd, out RECT rect);
    [DllImport("user32.dll")] private static extern bool ScreenToClient(IntPtr hwnd, ref POINT point);
    [DllImport("user32.dll")] private static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")] private static extern uint GetWindowThreadProcessId(IntPtr hwnd, out uint processId);
    [DllImport("kernel32.dll", CharSet=CharSet.Auto)] private static extern IntPtr GetModuleHandle(string moduleName);
}
'@
Add-Type -TypeDefinition $hookSource -ErrorAction Stop

$serviceName = 'PsychBeaconHost'
$logPath = Join-Path $env:ProgramData 'PsychBeacon\logs\host.log'
$menu = New-Object System.Windows.Forms.ContextMenuStrip
$statusItem = $menu.Items.Add('PsychBeacon Host: checking...')
$statusItem.Enabled = $false
$openItem = $menu.Items.Add('Open PsychBeacon')
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

$script:mainForm = $null
$script:computerList = $null
$script:clientStatus = $null
$script:inputSocket = $null
$script:inputHook = $null
$script:inputTimer = $null
$script:viewerProcess = $null
$script:connectedHost = $null

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

function Test-PsychBeaconPeer([string]$hostName, [string]$address) {
    $udp = $null
    try {
        $udp = [System.Net.Sockets.UdpClient]::new()
        $udp.Client.ReceiveTimeout = 350
        $endpoint = [System.Net.IPEndPoint]::new([System.Net.IPAddress]::Parse($address), 43701)
        $bytes = [System.Text.Encoding]::ASCII.GetBytes('PSYBEACON_DISCOVER_V1')
        [void]$udp.Send($bytes, $bytes.Length, $endpoint)
        $remote = [System.Net.IPEndPoint]::new([System.Net.IPAddress]::Any, 0)
        $reply = [System.Text.Encoding]::UTF8.GetString($udp.Receive([ref]$remote))
        if (-not $reply.StartsWith('PSYBEACON_HOST_V1:')) { return $null }
        $reportedName = $reply.Substring('PSYBEACON_HOST_V1:'.Length)
        if ([string]::IsNullOrWhiteSpace($reportedName)) { $reportedName = $hostName }
        return [pscustomobject]@{ HostName = $reportedName; Address = $address; DisplayName = "$reportedName  ($address)" }
    } catch [System.Net.Sockets.SocketException] {
        return $null
    } catch {
        return $null
    } finally {
        if ($udp) { $udp.Dispose() }
    }
}

function Find-PsychBeaconPeers {
    $found = @{}
    $tailscale = Join-Path $env:ProgramFiles 'Tailscale\tailscale.exe'
    if (Test-Path -LiteralPath $tailscale) {
        try {
            $statusJson = (& $tailscale status --json 2>$null | Out-String)
            $status = $statusJson | ConvertFrom-Json -ErrorAction Stop
            foreach ($peer in $status.Peer.PSObject.Properties.Value) {
                if ($peer.Online -ne $true) { continue }
                $address = @($peer.TailscaleIPs | Where-Object { $_ -match '^\d+\.\d+\.\d+\.\d+$' } | Select-Object -First 1)
                if ($address.Count -eq 0) { continue }
                $candidate = Test-PsychBeaconPeer ([string]$peer.HostName) ([string]$address[0])
                if ($candidate) { $found[$candidate.Address] = $candidate }
            }
        } catch { }
    }

    $broadcast = $null
    try {
        $broadcast = [System.Net.Sockets.UdpClient]::new()
        $broadcast.EnableBroadcast = $true
        $broadcast.Client.ReceiveTimeout = 250
        $endpoint = [System.Net.IPEndPoint]::new([System.Net.IPAddress]::Broadcast, 43701)
        $request = [System.Text.Encoding]::ASCII.GetBytes('PSYBEACON_DISCOVER_V1')
        [void]$broadcast.Send($request, $request.Length, $endpoint)
        $deadline = [DateTime]::UtcNow.AddMilliseconds(1000)
        while ([DateTime]::UtcNow -lt $deadline) {
            try {
                $remote = [System.Net.IPEndPoint]::new([System.Net.IPAddress]::Any, 0)
                $reply = [System.Text.Encoding]::UTF8.GetString($broadcast.Receive([ref]$remote))
                if ($reply.StartsWith('PSYBEACON_HOST_V1:')) {
                    $name = $reply.Substring('PSYBEACON_HOST_V1:'.Length)
                    $address = $remote.Address.ToString()
                    if (-not $found.ContainsKey($address)) {
                        $found[$address] = [pscustomobject]@{ HostName = $name; Address = $address; DisplayName = "$name  ($address)" }
                    }
                }
            } catch [System.Net.Sockets.SocketException] {
                if ($_.Exception.SocketErrorCode -ne [System.Net.Sockets.SocketError]::TimedOut -and $_.Exception.SocketErrorCode -ne [System.Net.Sockets.SocketError]::WouldBlock) { break }
            }
        }
    } catch { } finally { if ($broadcast) { $broadcast.Dispose() } }
    return @($found.Values | Sort-Object HostName)
}

function Update-ClientStatus([string]$message) {
    if ($script:clientStatus) { $script:clientStatus.Text = $message }
}

function Stop-PsychBeaconClient {
    if ($script:inputTimer) { $script:inputTimer.Stop(); $script:inputTimer.Dispose(); $script:inputTimer = $null }
    if ($script:inputHook) { $script:inputHook.Dispose(); $script:inputHook = $null }
    if ($script:inputSocket) {
        try { if ($script:inputSocket.State -eq [System.Net.WebSockets.WebSocketState]::Open) { $script:inputSocket.CloseOutputAsync([System.Net.WebSockets.WebSocketCloseStatus]::NormalClosure, 'Disconnected', [Threading.CancellationToken]::None).GetAwaiter().GetResult() } } catch { }
        $script:inputSocket.Dispose()
        $script:inputSocket = $null
    }
    if ($script:viewerProcess) {
        try { if (-not $script:viewerProcess.HasExited) { $script:viewerProcess.CloseMainWindow(); if (-not $script:viewerProcess.WaitForExit(1500)) { $script:viewerProcess.Kill() } } } catch { }
        $script:viewerProcess.Dispose()
        $script:viewerProcess = $null
    }
    $script:connectedHost = $null
    Update-ClientStatus 'Disconnected'
    Update-MenuState
}

function Connect-PsychBeaconHost($computer) {
    Stop-PsychBeaconClient
    Update-ClientStatus "Connecting to $($computer.HostName)..."
    $script:mainForm.UseWaitCursor = $true
    [System.Windows.Forms.Application]::DoEvents()
    $udp = $null
    try {
        $udp = [System.Net.Sockets.UdpClient]::new(0)
        $udp.Client.ReceiveTimeout = 20000
        $server = [System.Net.IPEndPoint]::new([System.Net.IPAddress]::Parse($computer.Address), 43701)
        $request = [System.Text.Encoding]::ASCII.GetBytes('PSYBEACON_START_STREAM_V1:43702:1')
        [void]$udp.Send($request, $request.Length, $server)
        $remote = [System.Net.IPEndPoint]::new([System.Net.IPAddress]::Any, 0)
        $reply = [System.Text.Encoding]::UTF8.GetString($udp.Receive([ref]$remote))
        $prefix = 'PSYBEACON_STREAM_INFO_V1:'
        if (-not $reply.StartsWith($prefix)) { throw "Unexpected host reply: $reply" }
        $manifest = @($reply.Substring($prefix.Length) | ConvertFrom-Json -ErrorAction Stop)
        if ($manifest.Count -lt 1) { throw 'The host returned no display to view.' }
        $display = $manifest[0]

        $socket = [System.Net.WebSockets.ClientWebSocket]::new()
        $socket.ConnectAsync([Uri]("ws://{0}:{1}" -f $computer.Address, $display.sidecarPort), [Threading.CancellationToken]::None).GetAwaiter().GetResult()
        $script:inputSocket = $socket

        $ffplay = Join-Path $env:ProgramFiles 'PsychBeaconHost\ffmpeg\ffplay.exe'
        if (-not (Test-Path -LiteralPath $ffplay)) { throw "FFplay isn't installed at $ffplay. Re-run the PsychBeacon installer." }
        $arguments = '-hide_banner -loglevel warning -fflags nobuffer -flags low_delay -framedrop -f h264 -window_title "PsychBeacon — {0}" "udp://0.0.0.0:{1}"' -f $computer.HostName, $display.streamPort
        $script:viewerProcess = Start-Process -FilePath $ffplay -ArgumentList $arguments -PassThru -WindowStyle Hidden
        $script:connectedHost = $computer
        $script:inputHook = [PsychBeaconInputHook]::new($script:viewerProcess.Id)

        $script:inputTimer = New-Object System.Windows.Forms.Timer
        $script:inputTimer.Interval = 15
        $script:inputTimer.Add_Tick({
            if ($script:viewerProcess -and $script:viewerProcess.HasExited) { Stop-PsychBeaconClient; return }
            if (-not $script:inputHook -or -not $script:inputSocket -or $script:inputSocket.State -ne [System.Net.WebSockets.WebSocketState]::Open) { return }
            $sent = 0
            $inputEvent = $null
            while ($sent -lt 80 -and $script:inputHook.TryDequeue([ref]$inputEvent)) {
                $payload = switch ($inputEvent.Type) {
                    'mouseMove' { @{ type = 'mouseMove'; x = $inputEvent.X; y = $inputEvent.Y } }
                    'mouseDown' { @{ type = 'mouseDown'; button = $inputEvent.Button } }
                    'mouseUp' { @{ type = 'mouseUp'; button = $inputEvent.Button } }
                    'scroll' { @{ type = 'scroll'; deltaX = $inputEvent.DeltaX; deltaY = $inputEvent.DeltaY } }
                    'keyDown' { @{ type = 'keyDown'; keyCode = $inputEvent.KeyCode } }
                    'keyUp' { @{ type = 'keyUp'; keyCode = $inputEvent.KeyCode } }
                }
                if ($payload) {
                    $json = ConvertTo-Json -InputObject $payload -Compress
                    $bytes = [System.Text.Encoding]::UTF8.GetBytes($json)
                    $segment = [ArraySegment[byte]]::new($bytes)
                    try { $script:inputSocket.SendAsync($segment, [System.Net.WebSockets.WebSocketMessageType]::Text, $true, [Threading.CancellationToken]::None).GetAwaiter().GetResult() }
                    catch { Stop-PsychBeaconClient; return }
                }
                $sent++
            }
        })
        $script:inputTimer.Start()
        Update-ClientStatus "Connected to $($computer.HostName) — video and input active. Close the video window or click Disconnect to end the session."
    } catch {
        Stop-PsychBeaconClient
        [System.Windows.Forms.MessageBox]::Show("Couldn't connect to $($computer.HostName):`n`n$($_.Exception.Message)", 'PsychBeacon') | Out-Null
    } finally {
        if ($udp) { $udp.Dispose() }
        if ($script:mainForm) { $script:mainForm.UseWaitCursor = $false }
    }
}

function Show-PsychBeaconWindow {
    if (-not $script:mainForm) {
        $form = New-Object System.Windows.Forms.Form
        $form.Text = 'PsychBeacon'
        $form.StartPosition = 'CenterScreen'
        $form.ClientSize = New-Object System.Drawing.Size(720, 430)
        $form.MinimumSize = New-Object System.Drawing.Size(640, 390)

        $hostLabel = New-Object System.Windows.Forms.Label
        $hostLabel.Text = 'This PC'
        $hostLabel.Location = New-Object System.Drawing.Point(18, 16)
        $hostLabel.AutoSize = $true
        $form.Controls.Add($hostLabel)
        $script:serviceLabel = New-Object System.Windows.Forms.Label
        $script:serviceLabel.Location = New-Object System.Drawing.Point(18, 43)
        $script:serviceLabel.Size = New-Object System.Drawing.Size(300, 24)
        $form.Controls.Add($script:serviceLabel)

        $startHost = New-Object System.Windows.Forms.Button
        $startHost.Text = 'Start host'
        $startHost.Location = New-Object System.Drawing.Point(18, 76)
        $startHost.Size = New-Object System.Drawing.Size(110, 30)
        $startHost.Add_Click({ Start-Process -FilePath (Join-Path $env:SystemRoot 'System32\sc.exe') -Verb RunAs -ArgumentList @('start', $serviceName) | Out-Null; Update-MenuState })
        $form.Controls.Add($startHost)
        $stopHost = New-Object System.Windows.Forms.Button
        $stopHost.Text = 'Stop host'
        $stopHost.Location = New-Object System.Drawing.Point(140, 76)
        $stopHost.Size = New-Object System.Drawing.Size(110, 30)
        $stopHost.Add_Click({ Start-Process -FilePath (Join-Path $env:SystemRoot 'System32\sc.exe') -Verb RunAs -ArgumentList @('stop', $serviceName) | Out-Null; Update-MenuState })
        $form.Controls.Add($stopHost)

        $clientLabel = New-Object System.Windows.Forms.Label
        $clientLabel.Text = 'Connect to a computer'
        $clientLabel.Location = New-Object System.Drawing.Point(18, 128)
        $clientLabel.AutoSize = $true
        $form.Controls.Add($clientLabel)
        $computerList = New-Object System.Windows.Forms.ListBox
        $computerList.Location = New-Object System.Drawing.Point(18, 155)
        $computerList.Size = New-Object System.Drawing.Size(420, 220)
        $computerList.DisplayMember = 'DisplayName'
        $form.Controls.Add($computerList)
        $refresh = New-Object System.Windows.Forms.Button
        $refresh.Text = 'Refresh list'
        $refresh.Location = New-Object System.Drawing.Point(458, 155)
        $refresh.Size = New-Object System.Drawing.Size(220, 32)
        $form.Controls.Add($refresh)
        $script:connectButton = New-Object System.Windows.Forms.Button
        $script:connectButton.Text = 'Connect'
        $script:connectButton.Location = New-Object System.Drawing.Point(458, 198)
        $script:connectButton.Size = New-Object System.Drawing.Size(105, 34)
        $form.Controls.Add($script:connectButton)
        $script:disconnectButton = New-Object System.Windows.Forms.Button
        $script:disconnectButton.Text = 'Disconnect'
        $script:disconnectButton.Location = New-Object System.Drawing.Point(573, 198)
        $script:disconnectButton.Size = New-Object System.Drawing.Size(105, 34)
        $script:disconnectButton.Enabled = $false
        $form.Controls.Add($script:disconnectButton)
        $script:clientStatus = New-Object System.Windows.Forms.Label
        $script:clientStatus.Text = 'Disconnected'
        $script:clientStatus.Location = New-Object System.Drawing.Point(458, 246)
        $script:clientStatus.Size = New-Object System.Drawing.Size(220, 100)
        $form.Controls.Add($script:clientStatus)

        $refresh.Add_Click({
            $script:computerList.Items.Clear()
            $script:clientStatus.Text = 'Searching for compatible PsychBeacon hosts...'
            $script:mainForm.UseWaitCursor = $true
            [System.Windows.Forms.Application]::DoEvents()
            $peers = @(Find-PsychBeaconPeers)
            foreach ($peer in $peers) { [void]$script:computerList.Items.Add($peer) }
            $script:clientStatus.Text = if ($peers.Count) { "$($peers.Count) compatible computer(s) found." } else { 'No compatible computers found. Check that the remote host service is running.' }
            $script:mainForm.UseWaitCursor = $false
        })
        $script:connectButton.Add_Click({ if ($script:computerList.SelectedItem) { Connect-PsychBeaconHost $script:computerList.SelectedItem } })
        $script:disconnectButton.Add_Click({ Stop-PsychBeaconClient })
        $form.Add_FormClosing({
            if ($_.CloseReason -eq [System.Windows.Forms.CloseReason]::UserClosing) { $_.Cancel = $true; $script:mainForm.Hide() }
        })
        $uiTimer = New-Object System.Windows.Forms.Timer
        $uiTimer.Interval = 1000
        $uiTimer.Add_Tick({
            $service = Get-Service -Name $serviceName -ErrorAction SilentlyContinue
            $script:serviceLabel.Text = if ($service) { "Host service: $($service.Status)" } else { 'Host service: not installed' }
            $script:disconnectButton.Enabled = $null -ne $script:connectedHost
            $script:connectButton.Enabled = $null -eq $script:connectedHost
        })
        $uiTimer.Start()
        $script:mainForm = $form
        $script:computerList = $computerList
    }
    $script:mainForm.Show()
    $script:mainForm.WindowState = [System.Windows.Forms.FormWindowState]::Normal
    $script:mainForm.Activate()
}

$menu.add_Opening({ Update-MenuState })
$openItem.add_Click({ Show-PsychBeaconWindow })
$logsItem.add_Click({
    if (Test-Path -LiteralPath $logPath) { Start-Process notepad.exe -ArgumentList @($logPath) }
    else { [System.Windows.Forms.MessageBox]::Show("The host log doesn't exist yet: $logPath", 'PsychBeacon Host') | Out-Null }
})
$quitItem.add_Click({ Stop-PsychBeaconClient; $icon.Visible = $false; $icon.Dispose(); $menu.Dispose(); [System.Windows.Forms.Application]::Exit() })
$icon.add_DoubleClick({ Show-PsychBeaconWindow })

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

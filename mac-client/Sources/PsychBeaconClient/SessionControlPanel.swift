import AppKit
import SwiftUI

@MainActor
private struct SessionControlView: View {
    let hostName: String
    let onDisconnect: () -> Void

    var body: some View {
        HStack(spacing: 12) {
            Circle().fill(.green).frame(width: 8, height: 8)
            Text("Connected to \(hostName)")
                .font(.callout.weight(.medium))
                .lineLimit(1)
            Button("Disconnect", role: .destructive, action: onDisconnect)
                .keyboardShortcut(.cancelAction)
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 10)
        .background(.regularMaterial, in: Capsule())
    }
}

/// Small floating session control, kept above stream windows so disconnect
/// remains available even when the computer picker is behind them.
@MainActor
final class SessionControlPanelController {
    private let panel: NSPanel
    private let onDisconnect: () -> Void

    init(onDisconnect: @escaping () -> Void) {
        self.onDisconnect = onDisconnect
        panel = NSPanel(
            contentRect: NSRect(x: 0, y: 0, width: 360, height: 48),
            styleMask: [.titled, .utilityWindow, .nonactivatingPanel],
            backing: .buffered,
            defer: false
        )
        panel.title = "PsychBeacon Session"
        panel.titleVisibility = .hidden
        panel.titlebarAppearsTransparent = true
        panel.isMovableByWindowBackground = true
        panel.isFloatingPanel = true
        panel.level = .floating
        panel.hidesOnDeactivate = false
        panel.isReleasedWhenClosed = false
        panel.collectionBehavior = [.canJoinAllSpaces, .stationary, .ignoresCycle]
        panel.standardWindowButton(.closeButton)?.isHidden = true
        panel.standardWindowButton(.miniaturizeButton)?.isHidden = true
        panel.standardWindowButton(.zoomButton)?.isHidden = true
    }

    func show(hostName: String) {
        panel.contentView = NSHostingView(rootView: SessionControlView(
            hostName: hostName,
            onDisconnect: onDisconnect
        ))
        if let screen = NSScreen.main {
            panel.setFrameTopLeftPoint(NSPoint(
                x: screen.visibleFrame.midX - panel.frame.width / 2,
                y: screen.visibleFrame.maxY - 12
            ))
        }
        panel.orderFrontRegardless()
    }

    func hide() {
        panel.orderOut(nil)
    }
}

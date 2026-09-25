import Foundation

// Temporary CLI entry point for exercising module 1 (network discovery)
// during early development. Module 4 (macOS Client & Sidecar Portal) will
// replace this with a real SwiftUI/AppKit app lifecycle that hands the
// resolved HostRoute to the VideoToolbox/Metal streaming session instead of
// just printing it.

let discovery = NetworkDiscovery()

Task {
    do {
        let route = try await discovery.resolveHostRoute()
        switch route {
        case .lan(let host, let port):
            print("Found host on LAN at \(host):\(port) — using direct P2P connection")
        case .tailscale(let host, let port):
            print("No LAN host found; routing via Tailscale mesh IP \(host):\(port)")
        }
    } catch {
        print("Discovery failed: \(error)")
    }
    exit(0)
}

RunLoop.main.run()

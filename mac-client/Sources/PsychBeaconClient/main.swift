import Cocoa

// Module 1's discovery CLI test moved out of the app's entry point — it now
// lives in NetworkDiscovery.swift, unchanged, ready to be called from
// AppDelegate once module 4 replaces the file-based test harness with the
// real network path. This entry point boots the actual windowed app.

let app = NSApplication.shared
let delegate = AppDelegate()
app.delegate = delegate
app.setActivationPolicy(.regular)
app.run()

import AVFoundation
import Cocoa
import MetalKit
import SwiftUI

/// Module 4's window + decode + render setup, wired to the real network
/// path: `NetworkDiscovery` resolves the host, `DisplayNegotiator` requests
/// N virtual displays and gets back a manifest, then one
/// `DisplayWindowController` per entry owns that display's independent
/// window/decode/render/input pipeline.
///
/// **Single-display path verified working end-to-end 2026-09-26, over a
/// real Tailscale mesh connection — not localhost, not a simulation.** Mac
/// and Windows host on different physical subnets (confirmed: LAN broadcast
/// discovery correctly failed, since broadcast doesn't cross subnets —
/// that's not a bug, see `NetworkDiscovery.swift`), connected instead via
/// Tailscale (`PSYBEACON_TARGET_HOST` env var — see that file's doc comment
/// for why the target has to be named explicitly on a real multi-device
/// tailnet). The full chain confirmed working: discovery → start-stream
/// request over the Tailscale IP → host adds a display, captures,
/// NVENC-encodes → raw H.264 over UDP across the actual WireGuard tunnel →
/// `NALUnitParser` reassembles it → `VideoDecoder` decodes → `MetalRenderer`
/// renders → window shows the real remote desktop.
///
/// **Multi-display orchestration (`DisplayNegotiator`,
/// `DisplayWindowController`) added 2026-09-26, unverified** — same
/// "written blind, ask the compiler/runtime for the real feedback" approach
/// that worked for the rest of this module, just not yet exercised against
/// a host that actually adds more than one display in a session.
/// `PSYBEACON_DISPLAY_COUNT` env var controls how many are requested
/// (default 1, matching the already-verified single-display path exactly).
///
/// Pass a file path as the first command-line argument to fall back to
/// the file-based test harness (`playTestFile`) for regression-testing
/// decode/render in isolation from the network — this path is unaffected
/// by the multi-display change and still uses one ad hoc window.
///
/// Decode/render verification notes (2026-09-25, built and ran on macOS,
/// Swift 5.9 toolchain, this repo's macOS 13 target): decoding an ffmpeg
/// `testsrc` test pattern rendered correctly (colors, motion, the
/// pattern's counter all correct — not just "a window appeared"). Two
/// things worth knowing for next time this needs testing:
/// - `MTLCreateSystemDefaultDevice`/`NSApplication` need an actual
///   interactive WindowServer session. Run in a real Terminal window, not
///   through an automation/agent context that executes commands outside
///   an interactive GUI session (confirmed: it builds and launches fine
///   that way, then hangs completely silently — no crash, no output, not
///   even this file's own synchronous `print` calls — because
///   `applicationDidFinishLaunching` never actually fires).
/// - `swift build`/`swift run` in this package don't need Xcode installed,
///   just the Swift toolchain + macOS SDK.
final class AppDelegate: NSObject, NSApplicationDelegate {
    /// First stream port requested from the host; display `i` uses
    /// `basePort + i` (see `DisplayNegotiator`). TODO(module 4):
    /// negotiate/randomize rather than hardcode, once there's a reason to
    /// (e.g. running two clients on the same machine).
    private let baseStreamReceivePort: UInt16 = 43702

    private var displayControllers: [DisplayWindowController] = []
    private var pickerWindow: NSWindow?
    private var pickerModel: HostPickerModel?

    // Only used by the file-based test harness (`playTestFile`), which has
    // no manifest and thus no `DisplayWindowController` to own these.
    private var window: NSWindow!
    private var mtkView: MTKView!
    private var renderer: MetalRenderer!
    private var decoder: VideoDecoder!

    func applicationDidFinishLaunching(_ notification: Notification) {
        if let path = CommandLine.arguments.dropFirst().first {
            print("File argument given — using the local test harness, not the network.")
            setUpTestHarnessWindow()
            playTestFile(at: path)
        } else {
            showComputerPicker()
        }
    }

    private func showComputerPicker() {
        let model = HostPickerModel()
        pickerModel = model
        let content = HostPickerView(model: model) { [weak self, weak model] computer, displayCount in
            guard let self, let model else { return }
            self.connectToHost(computer, displayCount: displayCount, model: model)
        }

        let window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 700, height: 500),
            styleMask: [.titled, .closable, .miniaturizable, .resizable],
            backing: .buffered,
            defer: false
        )
        window.title = "PsychBeacon"
        window.contentView = NSHostingView(rootView: content)
        window.center()
        window.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)
        pickerWindow = window
    }

    private func setUpTestHarnessWindow() {
        guard let device = MTLCreateSystemDefaultDevice() else {
            fatalError("No Metal-capable GPU found")
        }

        mtkView = MTKView(frame: NSRect(x: 0, y: 0, width: 1280, height: 720), device: device)
        mtkView.colorPixelFormat = .bgra8Unorm
        mtkView.preferredFramesPerSecond = 60
        mtkView.isPaused = true
        mtkView.enableSetNeedsDisplay = true

        do {
            renderer = try MetalRenderer(device: device)
        } catch {
            fatalError("MetalRenderer init failed: \(error)")
        }
        mtkView.delegate = renderer

        window = NSWindow(
            contentRect: mtkView.frame,
            styleMask: [.titled, .closable, .resizable, .miniaturizable],
            backing: .buffered,
            defer: false
        )
        window.title = "PsychBeacon — decode/render test"
        window.contentView = mtkView
        window.center()
        window.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)

        decoder = VideoDecoder()
        decoder.onDecodedFrame = { [weak self] pixelBuffer in
            guard let self else { return }
            self.renderer.present(pixelBuffer)
            DispatchQueue.main.async {
                self.mtkView.needsDisplay = true
            }
        }
    }

    private func connectToHost(
        _ computer: DiscoveredComputer,
        displayCount: Int,
        model: HostPickerModel
    ) {
        model.beginConnecting(to: computer)
        Task { @MainActor in
            do {
                let hostAddress = computer.address
                let manifest = try await DisplayNegotiator().negotiate(
                    hostAddress: hostAddress,
                    hostControlPort: computer.port,
                    basePort: baseStreamReceivePort,
                    displayCount: displayCount
                )
                print("Negotiated \(manifest.count) display(s) with the host: \(manifest)")

                guard let device = MTLCreateSystemDefaultDevice() else {
                    fatalError("No Metal-capable GPU found")
                }

                for info in manifest {
                    let controller = try DisplayWindowController(
                        info: info, hostAddress: hostAddress, device: device
                    )
                    displayControllers.append(controller)
                }

                model.connectionSucceeded(to: computer, displayCount: manifest.count)
                NSApp.activate(ignoringOtherApps: true)
            } catch {
                model.connectionFailed(error)
                print("Couldn't connect to \(computer.hostName): \(error)")
            }
        }
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool {
        true
    }

    private func playTestFile(at path: String) {
        let url = URL(fileURLWithPath: path)
        let asset = AVAsset(url: url)

        Task {
            do {
                guard let track = try await asset.loadTracks(withMediaType: .video).first else {
                    print("No video track found in \(path)")
                    return
                }

                let reader = try AVAssetReader(asset: asset)
                // nil settings = passthrough: still-compressed H.264 sample
                // buffers, not re-decoded by AVFoundation — VTDecompressionSession
                // needs to be the one doing the actual decode.
                let output = AVAssetReaderTrackOutput(track: track, outputSettings: nil)
                reader.add(output)
                reader.startReading()

                var configured = false
                var frameCount = 0

                while let sampleBuffer = output.copyNextSampleBuffer() {
                    if !configured, let formatDescription = CMSampleBufferGetFormatDescription(sampleBuffer) {
                        try decoder.configure(formatDescription: formatDescription)
                        configured = true
                        print("VideoDecoder configured from stream format description")
                    }
                    decoder.decode(sampleBuffer: sampleBuffer)
                    frameCount += 1
                    // Paced to roughly 30fps so this is watchable rather
                    // than flashing past in a fraction of a second — not
                    // meant to survive into the real streaming path, which
                    // will be paced by frame arrival over the network, not
                    // a sleep.
                    try? await Task.sleep(nanoseconds: 33_000_000)
                }

                print("Finished reading \(path): \(frameCount) sample buffers")
            } catch {
                print("Failed to read \(path): \(error)")
            }
        }
    }
}

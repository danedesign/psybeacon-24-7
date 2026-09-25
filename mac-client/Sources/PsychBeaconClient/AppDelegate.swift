import AVFoundation
import Cocoa
import MetalKit

/// Module 4's window + decode + render setup, now wired to the real
/// network path: `NetworkDiscovery` resolves the host (module 1, unchanged
/// since it was written — this is the first time anything actually calls
/// it), then `NetworkStreamReceiver` requests and receives the live NVENC
/// stream module 3 already proved works end-to-end on the Windows side.
///
/// Untested — unlike the file-based decode/render path this replaces
/// (verified 2026-09-25 against an ffmpeg test pattern; see that
/// verification's notes preserved below), nothing about
/// `NetworkStreamReceiver`/`NALUnitParser` or `NetworkDiscovery`'s actual
/// runtime behavior has been exercised yet. Pass a file path as the first
/// command-line argument to fall back to the old file-based test harness
/// (`playTestFile`, kept for exactly this — regression-testing decode/render
/// in isolation from the network) instead of the network path.
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
    /// Fixed for now — arbitrary, just needs to be free and reachable from
    /// the host on whichever route NetworkDiscovery resolved. TODO(module
    /// 4): negotiate/randomize rather than hardcode, once there's a reason
    /// to (e.g. running two clients on the same machine).
    private let localStreamReceivePort: UInt16 = 43702

    private var window: NSWindow!
    private var mtkView: MTKView!
    private var renderer: MetalRenderer!
    private var decoder: VideoDecoder!
    private var streamReceiver: NetworkStreamReceiver!

    func applicationDidFinishLaunching(_ notification: Notification) {
        guard let device = MTLCreateSystemDefaultDevice() else {
            fatalError("No Metal-capable GPU found")
        }

        mtkView = MTKView(frame: NSRect(x: 0, y: 0, width: 1280, height: 720), device: device)
        mtkView.colorPixelFormat = .bgra8Unorm
        mtkView.preferredFramesPerSecond = 60

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

        if let path = CommandLine.arguments.dropFirst().first {
            print("File argument given — using the local test harness, not the network.")
            playTestFile(at: path)
        } else {
            connectToHost()
        }
    }

    private func connectToHost() {
        streamReceiver = NetworkStreamReceiver(decoder: decoder)

        Task {
            do {
                let route = try await NetworkDiscovery().resolveHostRoute()
                let hostAddress: String
                let hostControlPort: UInt16
                switch route {
                case .lan(let host, let port):
                    print("Found host on LAN at \(host):\(port)")
                    (hostAddress, hostControlPort) = (host, port)
                case .tailscale(let host, let port):
                    print("No LAN host found; routing via Tailscale mesh IP \(host):\(port)")
                    (hostAddress, hostControlPort) = (host, port)
                }

                try streamReceiver.start(
                    hostAddress: hostAddress,
                    hostControlPort: hostControlPort,
                    localReceivePort: localStreamReceivePort
                )
            } catch {
                print(
                    "Couldn't connect to a host: \(error). Pass a local .h264/.ts/.mp4 file path "
                        + "as an argument to test decode/render without a live host instead."
                )
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

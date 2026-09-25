import AVFoundation
import Cocoa
import MetalKit

/// First testable slice of module 4: prove VideoToolbox decode + Metal
/// render work at all, against a real captured stream, before touching
/// networking. Pass a path to an .h264/.ts/.mp4 file as the first
/// command-line argument.
///
/// Verified working 2026-09-25: built and ran on macOS (Swift 5.9 toolchain,
/// this repo's macOS 13 target), decoding an ffmpeg `testsrc` test pattern
/// and rendering it correctly (colors, motion, the pattern's counter all
/// correct — not just "a window appeared"). Two things worth knowing for
/// next time this needs testing:
/// - `MTLCreateSystemDefaultDevice`/`NSApplication` need an actual
///   interactive WindowServer session. Run in a real Terminal window, not
///   through an automation/agent context that executes commands outside
///   an interactive GUI session (confirmed: it builds and launches fine
///   that way, then hangs completely silently — no crash, no output, not
///   even this file's own synchronous `print` calls — because
///   `applicationDidFinishLaunching` never actually fires).
/// - `swift build`/`swift run` in this package don't need Xcode installed,
///   just the Swift toolchain + macOS SDK.
///
/// TODO(module 4): replace `playTestFile` with the real path — read from
/// the network (module 1's discovered `HostRoute`) instead of a local file,
/// and drive `VideoDecoder.configure` from the stream's own SPS/PPS instead
/// of `AVAssetReader` handing us a ready-made format description.
final class AppDelegate: NSObject, NSApplicationDelegate {
    private var window: NSWindow!
    private var mtkView: MTKView!
    private var renderer: MetalRenderer!
    private var decoder: VideoDecoder!

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

        guard let path = CommandLine.arguments.dropFirst().first else {
            print("Usage: PsychBeaconClient <path to .h264/.ts test file>")
            return
        }
        playTestFile(at: path)
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

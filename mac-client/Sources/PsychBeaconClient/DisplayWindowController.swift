import Cocoa
import MetalKit

/// Everything one virtual display needs on the client: its own window,
/// its own decode/render pipeline, its own video receiver, and its own
/// input sidecar connection — so N displays from one `DisplayNegotiator`
/// manifest become N fully independent windows, not one window juggling
/// multiple streams.
///
/// **Unverified — written without a macOS toolchain available.** Follows
/// the exact per-component wiring `AppDelegate` used for the single-display
/// case (that path compiled and ran correctly on the first real attempt),
/// just parameterized per `DisplayInfo` instead of hardcoded.
@MainActor
final class DisplayWindowController {
    let info: DisplayInfo

    private var window: NSWindow!
    private var mtkView: InputCaptureView!
    private var renderer: MetalRenderer!
    private var decoder: VideoDecoder!
    private var streamReceiver: NetworkStreamReceiver!
    private var inputSidecar: InputSidecar!

    init(info: DisplayInfo, hostAddress: String, device: MTLDevice) throws {
        self.info = info

        mtkView = InputCaptureView(
            frame: NSRect(x: 0, y: 0, width: info.width, height: info.height),
            device: device
        )
        mtkView.colorPixelFormat = .bgra8Unorm
        mtkView.preferredFramesPerSecond = 60
        mtkView.isPaused = true
        mtkView.enableSetNeedsDisplay = true

        renderer = try MetalRenderer(device: device)
        mtkView.delegate = renderer

        window = NSWindow(
            contentRect: mtkView.frame,
            styleMask: [.titled, .closable, .resizable, .miniaturizable],
            backing: .buffered,
            defer: false
        )
        window.title = "PsychBeacon — display \(info.index)"
        window.contentView = mtkView
        // Stagger each display's window rather than stacking them all on
        // top of each other at the same default position.
        window.setFrameTopLeftPoint(
            NSPoint(x: 80 + CGFloat(info.index) * 60, y: NSScreen.main.map { $0.frame.maxY - 80 } ?? 600)
        )
        window.makeKeyAndOrderFront(nil)
        window.makeFirstResponder(mtkView)

        decoder = VideoDecoder()
        decoder.onDecodedFrame = { [weak self] pixelBuffer in
            guard let self else { return }
            self.renderer.present(pixelBuffer)
            DispatchQueue.main.async {
                self.mtkView.needsDisplay = true
            }
        }

        inputSidecar = InputSidecar()
        mtkView.sidecar = inputSidecar

        streamReceiver = NetworkStreamReceiver(decoder: decoder)
        try streamReceiver.start(localReceivePort: info.streamPort)
        inputSidecar.connect(
            hostAddress: hostAddress,
            port: info.sidecarPort,
            syncClipboard: info.index == 0
        )

        print(
            "DisplayWindowController: display \(info.index) ready "
                + "(\(info.width)x\(info.height) at (\(info.left), \(info.top)), "
                + "stream port \(info.streamPort), sidecar port \(info.sidecarPort))"
        )
    }

    func stop() {
        streamReceiver.stop()
        inputSidecar.disconnect()
        window.close()
    }
}

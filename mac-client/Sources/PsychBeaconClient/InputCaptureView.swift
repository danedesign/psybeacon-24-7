import AppKit
import MetalKit

/// `MTKView` subclass that captures mouse/keyboard events off the
/// rendered remote-desktop window and forwards them to `InputSidecar`.
///
/// Coordinates: AppKit delivers mouse events in this view's own coordinate
/// space, origin bottom-left unless `isFlipped` is overridden (it isn't
/// here, so the y-flip below is still needed) — normalized against the
/// view's *current* bounds (not the remote display's resolution), since
/// `sidecar.rs` on the host maps `[0, 1]` onto its own display's actual
/// pixel bounds regardless of what size this window happens to be.
final class InputCaptureView: MTKView {
    var sidecar: InputSidecar?

    private var trackingArea: NSTrackingArea?

    override var acceptsFirstResponder: Bool { true }

    override func becomeFirstResponder() -> Bool {
        true
    }

    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        if let trackingArea {
            removeTrackingArea(trackingArea)
        }
        let area = NSTrackingArea(
            rect: bounds,
            options: [.mouseMoved, .activeInKeyWindow, .inVisibleRect],
            owner: self,
            userInfo: nil
        )
        addTrackingArea(area)
        trackingArea = area
    }

    private func normalized(_ event: NSEvent) -> (x: Double, y: Double) {
        let point = convert(event.locationInWindow, from: nil)
        let x = Double(point.x / bounds.width)
        let y = Double(1.0 - (point.y / bounds.height)) // flip: AppKit is bottom-left, host expects top-left
        return (x, y)
    }

    override func mouseMoved(with event: NSEvent) {
        let p = normalized(event)
        sidecar?.mouseMove(x: p.x, y: p.y)
    }

    override func mouseDragged(with event: NSEvent) {
        let p = normalized(event)
        sidecar?.mouseMove(x: p.x, y: p.y)
    }

    override func rightMouseDragged(with event: NSEvent) {
        let p = normalized(event)
        sidecar?.mouseMove(x: p.x, y: p.y)
    }

    override func mouseDown(with event: NSEvent) {
        sidecar?.mouseDown(button: "left")
    }

    override func mouseUp(with event: NSEvent) {
        sidecar?.mouseUp(button: "left")
    }

    override func rightMouseDown(with event: NSEvent) {
        sidecar?.mouseDown(button: "right")
    }

    override func rightMouseUp(with event: NSEvent) {
        sidecar?.mouseUp(button: "right")
    }

    override func otherMouseDown(with event: NSEvent) {
        sidecar?.mouseDown(button: "middle")
    }

    override func otherMouseUp(with event: NSEvent) {
        sidecar?.mouseUp(button: "middle")
    }

    override func scrollWheel(with event: NSEvent) {
        sidecar?.scroll(deltaX: Double(event.scrollingDeltaX), deltaY: Double(event.scrollingDeltaY))
    }

    override func keyDown(with event: NSEvent) {
        guard let vk = KeycodeMap.windowsVirtualKey(forMacKeyCode: event.keyCode) else {
            print("InputCaptureView: no VK mapping for macOS keyCode \(event.keyCode), ignoring")
            return
        }
        sidecar?.keyDown(keyCode: vk)
    }

    override func keyUp(with event: NSEvent) {
        guard let vk = KeycodeMap.windowsVirtualKey(forMacKeyCode: event.keyCode) else {
            return
        }
        sidecar?.keyUp(keyCode: vk)
    }
}

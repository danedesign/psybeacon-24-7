import Foundation

/// Sends keyboard/mouse input to the host's sidecar WebSocket server
/// (`sidecar.rs` on the Windows side) — the spec's parallel control
/// channel alongside the video stream.
///
/// Fixed port (43703): the host runs this as a server, so the client just
/// connects once it already knows the host's address from discovery — no
/// negotiation message needed, unlike the video stream's port (which the
/// client picks and has to tell the host about via the start-stream
/// request).
///
/// `URLSessionWebSocketTask` (native, macOS 10.15+) rather than a
/// third-party library — matches this project's preference for Apple's
/// own frameworks over dependencies wherever they cover the need.
final class InputSidecar {
    private var session: URLSession?
    private var task: URLSessionWebSocketTask?
    private let mouseMoveLock = NSLock()
    private var pendingMouseMove: [String: Any]?
    private var mouseMoveScheduled = false

    func connect(hostAddress: String, port: UInt16 = 43703) {
        guard let url = URL(string: "ws://\(hostAddress):\(port)") else {
            print("InputSidecar: invalid URL for \(hostAddress):\(port)")
            return
        }
        let session = URLSession(configuration: .default)
        self.session = session
        let task = session.webSocketTask(with: url)
        self.task = task
        task.resume()
        print("InputSidecar: connecting to ws://\(hostAddress):\(port)")
        receiveLoop()
    }

    func disconnect() {
        task?.cancel(with: .goingAway, reason: nil)
        task = nil
        session = nil
        mouseMoveLock.lock()
        pendingMouseMove = nil
        mouseMoveScheduled = false
        mouseMoveLock.unlock()
    }

    /// The host never sends anything back — this only exists to notice
    /// when the connection dies (a `.failure` result), since nothing else
    /// would otherwise surface that.
    private func receiveLoop() {
        task?.receive { [weak self] result in
            switch result {
            case .success:
                self?.receiveLoop()
            case .failure(let error):
                print("InputSidecar: connection ended: \(error)")
            }
        }
    }

    private func send(_ payload: [String: Any]) {
        guard let data = try? JSONSerialization.data(withJSONObject: payload),
            let text = String(data: data, encoding: .utf8)
        else { return }
        task?.send(.string(text)) { error in
            if let error {
                print("InputSidecar: send failed: \(error)")
            }
        }
    }

    private func flushPendingMouseMove() {
        mouseMoveLock.lock()
        let payload = pendingMouseMove
        pendingMouseMove = nil
        mouseMoveScheduled = false
        mouseMoveLock.unlock()
        if let payload { send(payload) }
    }

    /// `x`/`y` normalized to `[0, 1]` relative to the video frame, not
    /// window pixels — the host maps these onto its own display's actual
    /// bounds (which may not be at position (0, 0), and is a different
    /// resolution than whatever size the window happens to be).
    func mouseMove(x: Double, y: Double) {
        mouseMoveLock.lock()
        pendingMouseMove = ["type": "mouseMove", "x": x, "y": y]
        guard !mouseMoveScheduled else {
            mouseMoveLock.unlock()
            return
        }
        mouseMoveScheduled = true
        mouseMoveLock.unlock()

        // Coalesce high-rate mouse events to the newest position at 60 Hz.
        // This avoids building a queue of stale cursor positions during drags.
        DispatchQueue.main.asyncAfter(deadline: .now() + (1.0 / 60.0)) { [weak self] in
            guard let self else { return }
            self.mouseMoveLock.lock()
            let payload = self.pendingMouseMove
            self.pendingMouseMove = nil
            self.mouseMoveScheduled = false
            self.mouseMoveLock.unlock()
            if let payload { self.send(payload) }
        }
    }

    func mouseDown(button: String) {
        flushPendingMouseMove()
        send(["type": "mouseDown", "button": button])
    }

    func mouseUp(button: String) {
        send(["type": "mouseUp", "button": button])
    }

    func scroll(deltaX: Double, deltaY: Double) {
        send(["type": "scroll", "deltaX": deltaX, "deltaY": deltaY])
    }

    func zoom(delta: Double) {
        send(["type": "zoom", "delta": delta])
    }

    /// `keyCode` is a Windows virtual-key code, already translated from
    /// the macOS `NSEvent.keyCode` by `KeycodeMap` — the host just injects
    /// whatever it's given, no translation on that side.
    func keyDown(keyCode: UInt16) {
        send(["type": "keyDown", "keyCode": keyCode])
    }

    func keyUp(keyCode: UInt16) {
        send(["type": "keyUp", "keyCode": keyCode])
    }
}

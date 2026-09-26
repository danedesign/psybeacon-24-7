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

    /// `x`/`y` normalized to `[0, 1]` relative to the video frame, not
    /// window pixels — the host maps these onto its own display's actual
    /// bounds (which may not be at position (0, 0), and is a different
    /// resolution than whatever size the window happens to be).
    func mouseMove(x: Double, y: Double) {
        send(["type": "mouseMove", "x": x, "y": y])
    }

    func mouseDown(button: String) {
        send(["type": "mouseDown", "button": button])
    }

    func mouseUp(button: String) {
        send(["type": "mouseUp", "button": button])
    }

    func scroll(deltaX: Double, deltaY: Double) {
        send(["type": "scroll", "deltaX": deltaX, "deltaY": deltaY])
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

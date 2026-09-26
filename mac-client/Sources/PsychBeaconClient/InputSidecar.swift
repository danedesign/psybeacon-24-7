import AppKit
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
    private var clipboardTimer: DispatchSourceTimer?
    private var lastClipboardChangeCount: Int?
    private var clipboardSyncEnabled = false

    private let maxClipboardBytes = 1_048_576

    func connect(hostAddress: String, port: UInt16 = 43703, syncClipboard: Bool = true) {
        guard let url = URL(string: "ws://\(hostAddress):\(port)") else {
            print("InputSidecar: invalid URL for \(hostAddress):\(port)")
            return
        }
        let session = URLSession(configuration: .default)
        self.session = session
        let task = session.webSocketTask(with: url)
        self.task = task
        clipboardSyncEnabled = syncClipboard
        task.resume()
        print("InputSidecar: connecting to ws://\(hostAddress):\(port)")
        startClipboardMonitoring()
        receiveLoop()
    }

    func disconnect() {
        let oldTask = task
        let oldSession = session
        clipboardTimer?.cancel()
        clipboardTimer = nil
        oldTask?.cancel(with: .goingAway, reason: nil)
        task = nil
        session = nil
        oldSession?.invalidateAndCancel()
        mouseMoveLock.lock()
        pendingMouseMove = nil
        mouseMoveScheduled = false
        mouseMoveLock.unlock()
        lastClipboardChangeCount = nil
    }

    /// Receives host clipboard updates and keeps reading so the callback
    /// reports sidecar disconnects to the client log.
    private func receiveLoop() {
        task?.receive { [weak self] result in
            switch result {
            case .success(let message):
                if case .string(let text) = message {
                    self?.handleIncoming(text)
                }
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

    private func startClipboardMonitoring() {
        guard clipboardSyncEnabled else { return }
        lastClipboardChangeCount = NSPasteboard.general.changeCount
        let timer = DispatchSource.makeTimerSource(queue: .main)
        timer.schedule(deadline: .now() + .milliseconds(500), repeating: .milliseconds(500))
        timer.setEventHandler { [weak self] in
            self?.sendClipboardIfChanged()
        }
        timer.resume()
        clipboardTimer = timer
    }

    private func sendClipboardIfChanged() {
        let pasteboard = NSPasteboard.general
        let changeCount = pasteboard.changeCount
        guard changeCount != lastClipboardChangeCount else { return }
        lastClipboardChangeCount = changeCount
        guard let text = pasteboard.string(forType: .string) else { return }
        guard text.lengthOfBytes(using: .utf8) <= maxClipboardBytes else {
            print("InputSidecar: ignoring clipboard text larger than 1 MiB")
            return
        }
        send(["type": "clipboard", "text": text])
    }

    private func handleIncoming(_ message: String) {
        guard clipboardSyncEnabled,
            let data = message.data(using: .utf8),
            let payload = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
            payload["type"] as? String == "clipboard",
            let text = payload["text"] as? String,
            text.lengthOfBytes(using: .utf8) <= maxClipboardBytes
        else { return }

        DispatchQueue.main.async { [weak self] in
            guard let self else { return }
            let pasteboard = NSPasteboard.general
            guard pasteboard.string(forType: .string) != text else {
                self.lastClipboardChangeCount = pasteboard.changeCount
                return
            }
            pasteboard.clearContents()
            pasteboard.setString(text, forType: .string)
            self.lastClipboardChangeCount = pasteboard.changeCount
            print("InputSidecar: received clipboard text from host")
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

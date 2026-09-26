import Foundation
#if canImport(Darwin)
import Darwin
#endif

/// One display in the manifest the host sends back after adding its virtual
/// displays — matches `DisplayInfo` in `windows-host/src/main.rs` field for
/// field (already camelCase on both sides, so no key-decoding strategy is
/// needed). `left`/`top` are this display's position on the host's virtual
/// desktop, needed by the input sidecar's coordinate mapping — not
/// predictable ahead of time, since the driver (not the client) decides
/// where an added display lands.
struct DisplayInfo: Decodable {
    let index: Int
    let streamPort: UInt16
    let sidecarPort: UInt16
    let width: Int
    let height: Int
    let left: Int
    let top: Int
}

enum DisplayNegotiatorError: Error {
    case socketCreationFailed
    case sendFailed
    case noReply
    case malformedReply(String)
}

/// Requests N virtual displays from the host and gets back where to find
/// each one: sends `PSYBEACON_START_STREAM_V1:<basePort>:<count>` to the
/// host's discovery port, then waits on the *same* socket (matching
/// `NetworkDiscovery.swift`'s send-then-recv-on-one-fd pattern) for the
/// `PSYBEACON_STREAM_INFO_V1:<json>` reply `run_multi_stream_session` sends
/// once every display it could add is up and its real bounds are known.
///
/// This negotiation happens once per connection, covering every display —
/// each display's actual video/input stream is then handled independently
/// by its own `NetworkStreamReceiver` + `InputSidecar` (see
/// `DisplayWindowController`).
struct DisplayNegotiator {
    var replyPrefix = "PSYBEACON_STREAM_INFO_V1:"
    var timeout: TimeInterval = 5.0

    /// Mirrors `NetworkDiscovery.broadcastForHost`'s pattern exactly: the
    /// actual work is a blocking socket call, so it runs on a background
    /// queue and bridges back via `withCheckedThrowingContinuation` rather
    /// than blocking whichever cooperative thread called this `async` func.
    func negotiate(
        hostAddress: String, hostControlPort: UInt16 = 43701, basePort: UInt16, displayCount: Int
    ) async throws -> [DisplayInfo] {
        try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<[DisplayInfo], Error>) in
            DispatchQueue.global(qos: .userInitiated).async {
                do {
                    continuation.resume(returning: try self.requestAndWait(
                        hostAddress: hostAddress, hostControlPort: hostControlPort,
                        basePort: basePort, displayCount: displayCount
                    ))
                } catch {
                    continuation.resume(throwing: error)
                }
            }
        }
    }

    /// Blocking; runs the send+recv on whichever thread calls `negotiate`.
    private func requestAndWait(
        hostAddress: String, hostControlPort: UInt16, basePort: UInt16, displayCount: Int
    ) throws -> [DisplayInfo] {
        let requestedCount = min(4, max(1, displayCount))
        // Windows adds displays sequentially and waits 2s for each new output
        // to settle before replying. Include room for driver/IOCTL overhead.
        let replyTimeout = max(timeout, TimeInterval(requestedCount) * 8.0)
        let fd = socket(AF_INET, SOCK_DGRAM, IPPROTO_UDP)
        guard fd >= 0 else { throw DisplayNegotiatorError.socketCreationFailed }
        defer { close(fd) }

        var rcvTimeout = timeval(
            tv_sec: Int(replyTimeout),
            tv_usec: Int32(replyTimeout.truncatingRemainder(dividingBy: 1) * 1_000_000)
        )
        setsockopt(fd, SOL_SOCKET, SO_RCVTIMEO, &rcvTimeout, socklen_t(MemoryLayout<timeval>.size))

        var addr = sockaddr_in()
        addr.sin_family = sa_family_t(AF_INET)
        addr.sin_port = in_port_t(hostControlPort.bigEndian)
        addr.sin_addr.s_addr = inet_addr(hostAddress)

        let message = "PSYBEACON_START_STREAM_V1:\(basePort):\(requestedCount)"
        let payload = Array(message.utf8)

        let sent = withUnsafePointer(to: &addr) { addrPtr -> Int in
            addrPtr.withMemoryRebound(to: sockaddr.self, capacity: 1) { sockaddrPtr in
                payload.withUnsafeBufferPointer { buf in
                    sendto(
                        fd, buf.baseAddress, buf.count, 0, sockaddrPtr,
                        socklen_t(MemoryLayout<sockaddr_in>.size)
                    )
                }
            }
        }
        guard sent > 0 else { throw DisplayNegotiatorError.sendFailed }
        print(
            "DisplayNegotiator: requested \(requestedCount) display(s) from \(hostAddress):\(hostControlPort), "
                + "base port \(basePort)"
        )

        // Each display takes ~2s to settle host-side before the manifest is
        // sent, so a multi-display request can take a while to reply —
        // retry the recv a few times within our overall `replyTimeout` rather
        // than treating one timed-out read as final.
        let deadline = Date().addingTimeInterval(replyTimeout)
        var replyBuffer = [UInt8](repeating: 0, count: 8192)
        while Date() < deadline {
            let received = recv(fd, &replyBuffer, replyBuffer.count, 0)
            if received > 0 {
                let reply = String(decoding: replyBuffer[0..<received], as: UTF8.self)
                guard reply.hasPrefix(replyPrefix) else {
                    throw DisplayNegotiatorError.malformedReply(reply)
                }
                let json = String(reply.dropFirst(replyPrefix.count))
                do {
                    return try JSONDecoder().decode([DisplayInfo].self, from: Data(json.utf8))
                } catch {
                    throw DisplayNegotiatorError.malformedReply("\(error): \(json)")
                }
            }
        }
        throw DisplayNegotiatorError.noReply
    }
}

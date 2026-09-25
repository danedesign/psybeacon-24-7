import CoreMedia
import Foundation
#if canImport(Darwin)
import Darwin
#endif

enum NetworkStreamError: Error {
    case socketCreationFailed
    case bindFailed
    case sendFailed
}

/// Requests the video stream from the host and receives it: sends
/// `PSYBEACON_START_STREAM_V1:<port>` to the host's discovery port (see
/// `main.rs`'s `run_discovery_responder` on the Windows side for the other
/// end of this), then listens on `port` for the raw H.264/UDP stream
/// `encode.rs` sends in response, feeding it through `NALUnitParser` into
/// `VideoDecoder`.
///
/// Raw BSD sockets, matching `NetworkDiscovery.swift`'s style — kept
/// consistent rather than introducing Network.framework as a second
/// approach, and this codebase's raw-socket patterns are the ones already
/// confirmed to compile here.
final class NetworkStreamReceiver {
    private let parser = NALUnitParser()
    private let decoder: VideoDecoder
    private var receiveSocket: Int32 = -1
    private var receiveThread: Thread?
    private var shouldStop = false

    init(decoder: VideoDecoder) {
        self.decoder = decoder
        parser.onFormatDescription = { [weak decoder] formatDescription in
            do {
                try decoder?.configure(formatDescription: formatDescription)
                print("NetworkStreamReceiver: decoder configured from the stream's SPS/PPS")
            } catch {
                print("NetworkStreamReceiver: decoder configure failed: \(error)")
            }
        }
        parser.onSampleBuffer = { [weak decoder] sampleBuffer in
            decoder?.decode(sampleBuffer: sampleBuffer)
        }
    }

    /// `hostAddress` and `hostControlPort` identify where to send the
    /// request — from module 1's resolved `HostRoute`, the discovery
    /// port either way (LAN or Tailscale, the host answers the same
    /// message identically on both). `localReceivePort` is the UDP port
    /// on this machine the host should stream *to*.
    func start(hostAddress: String, hostControlPort: UInt16 = 43701, localReceivePort: UInt16) throws {
        try startReceiving(on: localReceivePort)
        try sendStartStreamRequest(
            hostAddress: hostAddress,
            hostControlPort: hostControlPort,
            localReceivePort: localReceivePort
        )
    }

    func stop() {
        shouldStop = true
        if receiveSocket >= 0 {
            close(receiveSocket)
            receiveSocket = -1
        }
    }

    private func sendStartStreamRequest(
        hostAddress: String, hostControlPort: UInt16, localReceivePort: UInt16
    ) throws {
        let fd = socket(AF_INET, SOCK_DGRAM, IPPROTO_UDP)
        guard fd >= 0 else { throw NetworkStreamError.socketCreationFailed }
        defer { close(fd) }

        var addr = sockaddr_in()
        addr.sin_family = sa_family_t(AF_INET)
        addr.sin_port = in_port_t(hostControlPort.bigEndian)
        addr.sin_addr.s_addr = inet_addr(hostAddress)

        let message = "PSYBEACON_START_STREAM_V1:\(localReceivePort)"
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
        guard sent > 0 else { throw NetworkStreamError.sendFailed }
        print(
            "NetworkStreamReceiver: sent start-stream request to \(hostAddress):\(hostControlPort) "
                + "for local port \(localReceivePort)"
        )
    }

    private func startReceiving(on port: UInt16) throws {
        let fd = socket(AF_INET, SOCK_DGRAM, IPPROTO_UDP)
        guard fd >= 0 else { throw NetworkStreamError.socketCreationFailed }

        var addr = sockaddr_in()
        addr.sin_family = sa_family_t(AF_INET)
        addr.sin_port = in_port_t(port.bigEndian)
        addr.sin_addr.s_addr = InAddrAny

        let bindResult = withUnsafePointer(to: &addr) { addrPtr -> Int32 in
            addrPtr.withMemoryRebound(to: sockaddr.self, capacity: 1) { sockaddrPtr in
                bind(fd, sockaddrPtr, socklen_t(MemoryLayout<sockaddr_in>.size))
            }
        }
        guard bindResult == 0 else {
            close(fd)
            throw NetworkStreamError.bindFailed
        }

        receiveSocket = fd
        shouldStop = false

        let thread = Thread { [weak self] in
            self?.receiveLoop(fd: fd)
        }
        thread.name = "NetworkStreamReceiver.receive"
        thread.start()
        receiveThread = thread

        print("NetworkStreamReceiver: listening on UDP :\(port)")
    }

    private func receiveLoop(fd: Int32) {
        var buffer = [UInt8](repeating: 0, count: 65536)
        while !shouldStop {
            let received = recv(fd, &buffer, buffer.count, 0)
            guard received > 0 else {
                if shouldStop { break }
                continue
            }
            parser.feed(Data(buffer[0..<received]))
        }
    }
}

private let InAddrAny: in_addr_t = 0

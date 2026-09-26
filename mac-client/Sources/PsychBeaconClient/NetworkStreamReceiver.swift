import CoreMedia
import Foundation
#if canImport(Darwin)
import Darwin
#endif

enum NetworkStreamError: Error {
    case socketCreationFailed
    case bindFailed
}

/// Receives one display's video stream: listens on `port` for the raw
/// H.264/UDP stream `encode.rs` sends, feeding it through `NALUnitParser`
/// into `VideoDecoder`.
///
/// This class only receives — sending the initial
/// `PSYBEACON_START_STREAM_V1:<basePort>:<count>` request and negotiating
/// which port belongs to which display is `DisplayNegotiator`'s job (module
/// 4's multi-display orchestration needs one negotiation covering every
/// display, followed by N independent receivers, not N independent
/// requests).
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

    /// `localReceivePort` is the UDP port on this machine the host streams
    /// *to* — for a multi-display session, this is one specific display's
    /// `streamPort` from `DisplayNegotiator`'s manifest, not a value this
    /// class picks itself.
    func start(localReceivePort: UInt16) throws {
        try startReceiving(on: localReceivePort)
    }

    func stop() {
        shouldStop = true
        if receiveSocket >= 0 {
            close(receiveSocket)
            receiveSocket = -1
        }
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

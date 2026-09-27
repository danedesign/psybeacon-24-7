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
    private let stateLock = NSLock()
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
        stateLock.lock()
        shouldStop = true
        let fd = receiveSocket
        let thread = receiveThread
        receiveSocket = -1
        receiveThread = nil
        stateLock.unlock()

        guard fd >= 0 else { return }
        // Wake recv() first and let its owner leave the loop before closing
        // the descriptor. Closing a socket underneath recv() can race with
        // descriptor reuse during a fast disconnect/reconnect.
        _ = shutdown(fd, SHUT_RDWR)
        if let thread, thread !== Thread.current {
            while !thread.isFinished {
                Thread.sleep(forTimeInterval: 0.005)
            }
        }
        close(fd)
    }

    private func startReceiving(on port: UInt16) throws {
        let fd = socket(AF_INET, SOCK_DGRAM, IPPROTO_UDP)
        guard fd >= 0 else { throw NetworkStreamError.socketCreationFailed }

        var timeout = timeval(tv_sec: 0, tv_usec: 100_000)
        _ = setsockopt(
            fd,
            SOL_SOCKET,
            SO_RCVTIMEO,
            &timeout,
            socklen_t(MemoryLayout<timeval>.size)
        )

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

        stateLock.lock()
        receiveSocket = fd
        shouldStop = false
        stateLock.unlock()

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
        while !isStopping {
            let received = recv(fd, &buffer, buffer.count, 0)
            guard received > 0 else {
                if isStopping { break }
                continue
            }
            parser.feed(Data(buffer[0..<received]))
        }
    }

    private var isStopping: Bool {
        stateLock.lock()
        defer { stateLock.unlock() }
        return shouldStop
    }
}

private let InAddrAny: in_addr_t = 0

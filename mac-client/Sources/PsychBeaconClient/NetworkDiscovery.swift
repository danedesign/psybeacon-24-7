import Foundation
#if canImport(Darwin)
import Darwin
#endif

/// Where the PsychBeacon host was found and which transport should be used
/// to reach it.
enum HostRoute {
    case lan(host: String, port: UInt16)
    case tailscale(host: String, port: UInt16)
}

enum DiscoveryError: Error, CustomStringConvertible {
    case socketCreationFailed
    case hostNotFound
    /// Two genuinely different failure modes that both used to throw a
    /// bare, identical `tailscaleUnavailable` — found undiagnosable in
    /// practice: "same error" after fixing the peer's actual hostname
    /// gave no way to tell whether the CLI was even found at all versus
    /// found-but-no-matching-peer.
    case tailscaleBinaryNotFound
    case tailscaleNoMatchingPeer(searchedFor: String, peersSeen: [String])

    var description: String {
        switch self {
        case .socketCreationFailed: return "socketCreationFailed"
        case .hostNotFound: return "hostNotFound"
        case .tailscaleBinaryNotFound:
            return "tailscaleBinaryNotFound (checked /Applications/Tailscale.app, /usr/local/bin, /opt/homebrew/bin)"
        case .tailscaleNoMatchingPeer(let searchedFor, let peersSeen):
            let peerList = peersSeen.isEmpty ? "(none)" : peersSeen.joined(separator: ", ")
            return "tailscaleNoMatchingPeer(searched for hostname containing \"\(searchedFor)\", " +
                "peers actually seen: \(peerList))"
        }
    }
}

/// Startup discovery for module 1: broadcast a UDP ping on the local subnet
/// first (lowest latency, direct P2P path); if nothing answers, fall back to
/// the host's Tailscale mesh IP so the stream can still route over WireGuard.
struct NetworkDiscovery {
    var discoveryPort: UInt16 = 43701
    var timeout: TimeInterval = 1.5
    var magicRequest = "PSYBEACON_DISCOVER_V1"
    var magicReplyPrefix = "PSYBEACON_HOST_V1:"

    /// Which Tailscale peer counts as "the" host, when LAN discovery finds
    /// nothing and this falls back to the mesh — a substring matched
    /// case-insensitively against each peer's Tailscale `HostName`.
    /// Defaults to "psybeacon", which only ever matches a peer actually
    /// named that; override via `PSYBEACON_TARGET_HOST` for a real tailnet,
    /// where the host keeps its own machine hostname (confirmed testing
    /// against a tailnet with 8 devices, 4 of them Windows — matching "the"
    /// host by any generic heuristic isn't reliable there, and doesn't need
    /// to be once the target can just be named directly). E.g.:
    /// `PSYBEACON_TARGET_HOST=slsupercomputer swift run PsychBeaconClient`
    var tailscaleTargetHostnameSubstring: String =
        ProcessInfo.processInfo.environment["PSYBEACON_TARGET_HOST"] ?? "psybeacon"

    /// Call once at client startup. Throws if the host can't be found by
    /// either path; the caller should surface that (retry, or prompt for a
    /// manual host address) rather than silently hanging.
    func resolveHostRoute() async throws -> HostRoute {
        if let lan = try? await broadcastForHost() {
            return lan
        }
        return try await tailscaleHostRoute()
    }

    // MARK: - LAN broadcast

    private func broadcastForHost() async throws -> HostRoute {
        try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<HostRoute, Error>) in
            DispatchQueue.global(qos: .userInitiated).async {
                do {
                    continuation.resume(returning: try self.sendBroadcastAndWait())
                } catch {
                    continuation.resume(throwing: error)
                }
            }
        }
    }

    /// Blocking; always called from a background queue via `broadcastForHost`.
    ///
    /// Uses a raw BSD socket rather than Network.framework: NWConnection
    /// doesn't expose SO_BROADCAST, which a real subnet broadcast requires,
    /// so Network.framework UDP sends to 255.255.255.255 are unreliable.
    private func sendBroadcastAndWait() throws -> HostRoute {
        let fd = socket(AF_INET, SOCK_DGRAM, IPPROTO_UDP)
        guard fd >= 0 else { throw DiscoveryError.socketCreationFailed }
        defer { close(fd) }

        var broadcastEnable: Int32 = 1
        setsockopt(fd, SOL_SOCKET, SO_BROADCAST, &broadcastEnable, socklen_t(MemoryLayout<Int32>.size))

        var rcvTimeout = timeval(
            tv_sec: Int(timeout),
            tv_usec: Int32(timeout.truncatingRemainder(dividingBy: 1) * 1_000_000)
        )
        setsockopt(fd, SOL_SOCKET, SO_RCVTIMEO, &rcvTimeout, socklen_t(MemoryLayout<timeval>.size))

        var broadcastAddr = sockaddr_in()
        broadcastAddr.sin_family = sa_family_t(AF_INET)
        broadcastAddr.sin_port = in_port_t(discoveryPort.bigEndian)
        broadcastAddr.sin_addr.s_addr = inet_addr("255.255.255.255")

        let payload = Array(magicRequest.utf8)
        let sent = withUnsafePointer(to: &broadcastAddr) { addrPtr -> Int in
            addrPtr.withMemoryRebound(to: sockaddr.self, capacity: 1) { sockaddrPtr in
                payload.withUnsafeBufferPointer { buf in
                    sendto(fd, buf.baseAddress, buf.count, 0, sockaddrPtr, socklen_t(MemoryLayout<sockaddr_in>.size))
                }
            }
        }
        guard sent > 0 else { throw DiscoveryError.hostNotFound }

        var replyBuffer = [UInt8](repeating: 0, count: 512)
        var fromAddr = sockaddr_in()
        var fromLen = socklen_t(MemoryLayout<sockaddr_in>.size)

        let received = withUnsafeMutablePointer(to: &fromAddr) { fromPtr -> Int in
            fromPtr.withMemoryRebound(to: sockaddr.self, capacity: 1) { sockaddrPtr in
                recvfrom(fd, &replyBuffer, replyBuffer.count, 0, sockaddrPtr, &fromLen)
            }
        }
        guard received > 0 else { throw DiscoveryError.hostNotFound }

        let reply = String(decoding: replyBuffer[0..<received], as: UTF8.self)
        guard reply.hasPrefix(magicReplyPrefix) else { throw DiscoveryError.hostNotFound }

        // The host's address comes from the packet's source address, not the
        // payload, so the host doesn't need to know its own IP correctly.
        var addrBuf = [CChar](repeating: 0, count: Int(INET_ADDRSTRLEN))
        var sourceAddr = fromAddr
        inet_ntop(AF_INET, &sourceAddr.sin_addr, &addrBuf, socklen_t(INET_ADDRSTRLEN))

        return .lan(host: String(cString: addrBuf), port: discoveryPort)
    }

    // MARK: - Tailscale fallback

    private func tailscaleHostRoute() async throws -> HostRoute {
        let candidates = [
            "/Applications/Tailscale.app/Contents/MacOS/Tailscale",
            "/usr/local/bin/tailscale",
            "/opt/homebrew/bin/tailscale",
        ]
        guard let binaryPath = candidates.first(where: { FileManager.default.isExecutableFile(atPath: $0) }) else {
            throw DiscoveryError.tailscaleBinaryNotFound
        }

        let process = Process()
        process.executableURL = URL(fileURLWithPath: binaryPath)
        process.arguments = ["status", "--json"]

        let pipe = Pipe()
        process.standardOutput = pipe
        try process.run()
        process.waitUntilExit()

        let data = pipe.fileHandleForReading.readDataToEndOfFile()
        let status = try JSONDecoder().decode(TailscaleStatus.self, from: data)

        let needle = tailscaleTargetHostnameSubstring.lowercased()
        guard
            let peer = status.peers.first(where: { $0.hostName.lowercased().contains(needle) }),
            let ip = peer.tailscaleIPs.first
        else {
            throw DiscoveryError.tailscaleNoMatchingPeer(
                searchedFor: tailscaleTargetHostnameSubstring,
                peersSeen: status.peers.map(\.hostName)
            )
        }

        return .tailscale(host: ip, port: discoveryPort)
    }
}

/// Minimal model of `tailscale status --json` — only the fields we need.
/// Verify field names against the installed Tailscale version if this stops
/// matching; the local API's JSON shape isn't strictly versioned.
private struct TailscaleStatus: Decodable {
    let peers: [Peer]

    struct Peer: Decodable {
        let hostName: String
        let tailscaleIPs: [String]

        enum CodingKeys: String, CodingKey {
            case hostName = "HostName"
            case tailscaleIPs = "TailscaleIPs"
        }
    }

    enum CodingKeys: String, CodingKey {
        case peer = "Peer"
    }

    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        // `Peer` is `null` (not an empty object) when the tailnet has no
        // other devices — confirmed via a real decode failure testing
        // this against a Mac with no Tailscale peers.
        // `decodeIfPresent` handles both that and the key being absent
        // entirely; plain `decode` treated null as a hard error.
        let peerMap = try container.decodeIfPresent([String: Peer].self, forKey: .peer) ?? [:]
        peers = Array(peerMap.values)
    }
}

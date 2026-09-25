// swift-tools-version: 5.9
import PackageDescription

let package = Package(
    name: "PsychBeaconClient",
    platforms: [.macOS(.v13)],
    targets: [
        .executableTarget(
            name: "PsychBeaconClient",
            path: "Sources/PsychBeaconClient"
        )
    ]
)

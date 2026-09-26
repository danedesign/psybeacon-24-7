import SwiftUI

@MainActor
final class HostPickerModel: ObservableObject {
    @Published private(set) var computers: [DiscoveredComputer] = []
    @Published private(set) var isRefreshing = false
    @Published private(set) var isConnecting = false
    @Published var selectedAddress: String?
    @Published var statusMessage = "Looking for Windows computers running PsychBeacon…"

    func refresh() {
        guard !isRefreshing else { return }
        isRefreshing = true
        statusMessage = "Scanning your Tailscale network…"

        Task {
            defer { isRefreshing = false }
            do {
                computers = try await NetworkDiscovery().discoverCompatibleComputers()
                if computers.isEmpty {
                    selectedAddress = nil
                    statusMessage = "No compatible PsychBeacon hosts found. Check Tailscale and start the host app."
                } else {
                    let preferredHost = ProcessInfo.processInfo.environment["PSYBEACON_TARGET_HOST"]?
                        .trimmingCharacters(in: .whitespacesAndNewlines)
                        .lowercased()
                    let preferred = computers.first {
                        guard let preferredHost, !preferredHost.isEmpty else { return false }
                        return $0.hostName.lowercased().contains(preferredHost)
                    }
                    if let preferred {
                        selectedAddress = preferred.address
                    } else if !computers.contains(where: { $0.address == selectedAddress }) {
                        selectedAddress = computers.first?.address
                    }
                    statusMessage = "\(computers.count) compatible computer\(computers.count == 1 ? "" : "s") available"
                }
            } catch {
                computers = []
                selectedAddress = nil
                statusMessage = "Computer scan failed: \(error)"
            }
        }
    }

    func beginConnecting(to computer: DiscoveredComputer) {
        isConnecting = true
        statusMessage = "Connecting to \(computer.hostName)…"
    }

    func connectionSucceeded(to computer: DiscoveredComputer, displayCount: Int) {
        isConnecting = false
        statusMessage = "Connected to \(computer.hostName) · \(displayCount) display\(displayCount == 1 ? "" : "s")"
    }

    func connectionFailed(_ error: Error) {
        isConnecting = false
        statusMessage = "Connection failed: \(error)"
    }
}

@MainActor
struct HostPickerView: View {
    @ObservedObject var model: HostPickerModel
    let onConnect: @MainActor (DiscoveredComputer, Int) -> Void
    @State private var displayCount: Int = {
        let requested = ProcessInfo.processInfo.environment["PSYBEACON_DISPLAY_COUNT"]
            .flatMap(Int.init) ?? 1
        return min(4, max(1, requested))
    }()

    private var selectedComputer: DiscoveredComputer? {
        model.computers.first { $0.address == model.selectedAddress }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 18) {
            HStack(alignment: .top) {
                VStack(alignment: .leading, spacing: 5) {
                    Text("Computers")
                        .font(.largeTitle.weight(.semibold))
                    Text("Choose a Windows PC running the PsychBeacon host.")
                        .foregroundStyle(.secondary)
                }
                Spacer()
                Button(action: model.refresh) {
                    Label("Refresh", systemImage: "arrow.clockwise")
                }
                .disabled(model.isRefreshing || model.isConnecting)
            }

            List(selection: $model.selectedAddress) {
                ForEach(model.computers) { computer in
                    HStack(spacing: 12) {
                        Image(systemName: "desktopcomputer")
                            .font(.title2)
                            .foregroundStyle(.tint)
                            .frame(width: 34)
                        VStack(alignment: .leading, spacing: 3) {
                            Text(computer.hostName)
                                .font(.headline)
                            Text("PsychBeacon host ready · \(computer.address)")
                                .font(.subheadline)
                                .foregroundStyle(.secondary)
                        }
                        Spacer()
                        Circle()
                            .fill(.green)
                            .frame(width: 9, height: 9)
                            .accessibilityLabel("Available")
                    }
                    .padding(.vertical, 5)
                    .tag(computer.address)
                }
            }
            .listStyle(.inset)
            .overlay {
                if model.computers.isEmpty && !model.isRefreshing {
                    VStack(spacing: 8) {
                        Image(systemName: "desktopcomputer")
                            .font(.system(size: 30))
                            .foregroundStyle(.secondary)
                        Text("No compatible computers")
                            .font(.headline)
                        Text("Connect the Windows PC to Tailscale and start its PsychBeacon host.")
                            .font(.callout)
                            .foregroundStyle(.secondary)
                    }
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
                    .allowsHitTesting(false)
                }
            }

            HStack {
                Text(model.statusMessage)
                    .font(.callout)
                    .foregroundStyle(.secondary)
                    .lineLimit(2)
                Spacer(minLength: 12)
                Picker("Displays", selection: $displayCount) {
                    ForEach(1...4, id: \.self) { count in
                        Text("\(count)").tag(count)
                    }
                }
                .frame(width: 100)
                .disabled(model.isConnecting)

                Button("Connect") {
                    if let computer = selectedComputer {
                        onConnect(computer, displayCount)
                    }
                }
                .keyboardShortcut(.defaultAction)
                .disabled(selectedComputer == nil || model.isConnecting)
            }
        }
        .padding(22)
        .frame(minWidth: 620, minHeight: 430)
        .onAppear { model.refresh() }
    }
}

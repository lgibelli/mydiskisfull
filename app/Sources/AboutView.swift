// About: where the app accounts for its own cost. A monitor must never
// become the thing it hunts, so the watcher's measured cost is shown here
// against its budget, with the app's own.

import SwiftUI

struct AboutView: View {
    @ObservedObject var monitor: Monitor
    @State private var appCPU: Double = 0

    private var version: String {
        let v = Bundle.main.infoDictionary?["CFBundleShortVersionString"] as? String ?? "0"
        let b = Bundle.main.infoDictionary?["CFBundleVersion"] as? String ?? "0"
        return "Version \(v) (\(b))"
    }

    var body: some View {
        VStack(spacing: 12) {
            if let icon = NSApp.applicationIconImage {
                Image(nsImage: icon).resizable().frame(width: 72, height: 72)
            }
            Text("MyDiskIsFull").font(.title2.bold())
            Text(version).font(.caption).foregroundStyle(.secondary)
            Text("Watches your disk, predicts when it will be full, finds what fills it and helps it stay fixed.")
                .font(.callout).foregroundStyle(.secondary)
                .multilineTextAlignment(.center)
                .fixedSize(horizontal: false, vertical: true)
                .padding(.horizontal, 30)

            GroupBox {
                VStack(spacing: 10) {
                    if let c = monitor.status?.watcherCost {
                        HStack(spacing: 0) {
                            Stat(value: String(format: "%.2f s", Double(c.cpuUsPerHour) / 1e6),
                                 label: "watcher CPU an hour")
                            Divider().frame(height: 32)
                            Stat(value: Format.bytes(c.writtenPerHour), label: "it writes an hour")
                            Divider().frame(height: 32)
                            Stat(value: Format.bytes(monitor.status?.history.bytes ?? 0), label: "history")
                        }
                        Text(costNote(c)).font(.caption2).foregroundStyle(.secondary)
                            .multilineTextAlignment(.center)
                            .fixedSize(horizontal: false, vertical: true)
                    } else {
                        Text("The watcher's cost shows once it has run.").foregroundStyle(.secondary)
                    }
                    Divider()
                    Text(String(format: "This app has used %.0f ms of CPU since it opened %@ ago, including the commands it ran.",
                                appCPU, Format.duration(Int64(Date().timeIntervalSince(monitor.launched)))))
                        .font(.caption2).foregroundStyle(.secondary)
                        .multilineTextAlignment(.center)
                }
                .padding(.vertical, 6)
            }
            .padding(.horizontal, 20)

            Text("Free and open source, under the MIT or Apache 2.0 licence. No telemetry, no account.")
                .font(.caption2).foregroundStyle(.secondary)
            Link("Source", destination: URL(string: "https://github.com/lgibelli/mydiskisfull")!)
                .font(.caption)
        }
        .padding(24)
        .frame(width: 520)
        .onAppear { appCPU = monitor.ownCPUms }
    }

    private func costNote(_ c: WatcherCost) -> String {
        let within = c.cpuUsPerHour <= 1_000_000 && c.writtenPerHour <= 1_000_000
        var s = "Measured, at one sample every \(Format.duration(c.intervalSecs)): "
            + (within ? "within" : "over") + " its budget of 1 s of CPU and 1 MB of writes an hour."
        if c.periods == 0, c.catchUpCpuUs > 0 {
            s += " Still counting the first folder update after it started, which catches up on what changed meanwhile and costs more than the next ones."
        }
        return s
    }
}

private struct Stat: View {
    let value: String
    let label: String

    var body: some View {
        VStack(spacing: 3) {
            Text(value).font(.title3.monospacedDigit().weight(.medium))
            Text(label).font(.caption2).foregroundStyle(.secondary)
        }
        .frame(maxWidth: .infinity)
    }
}

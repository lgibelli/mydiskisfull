// The menu bar panel: a glance, not the app. How full the disk is, where
// it's heading, anything that needs attention, and the way into the main
// window, where everything else is.

import AppKit
import SwiftUI

struct StatusPanel: View {
    @ObservedObject var monitor: Monitor
    @Environment(\.openSettings) private var openSettings

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            if let s = monitor.status {
                header(s)
                    .padding(16)
                forecast(s)
                    .padding(.horizontal, 16)
                    .padding(.bottom, 14)
            } else {
                HStack(spacing: 10) {
                    if let e = monitor.statusError {
                        Image(systemName: "exclamationmark.triangle.fill").foregroundStyle(.orange)
                        Text(e).font(.callout).lineLimit(3)
                    } else {
                        ProgressView().controlSize(.small)
                        Text("Reading the history…").font(.callout).foregroundStyle(.secondary)
                    }
                }
                .padding(16)
            }
            attention
            Divider()
            footer
                .padding(12)
        }
        .frame(width: 340)
        .onAppear { monitor.viewed() }
        .task(id: monitor.lastRefresh) { await monitor.loadDetail() }
    }

    private func header(_ s: Status) -> some View {
        let used = Double(s.total - s.avail) / Double(max(s.total, 1))
        return HStack(spacing: 14) {
            CapacityRing(used: used, colors: monitor.health.ring, lineWidth: 7) {
                Text("\(Int((used * 100).rounded()))%")
                    .font(.system(size: 12, weight: .bold, design: .rounded))
                    .monospacedDigit()
            }
            .frame(width: 66, height: 66)
            VStack(alignment: .leading, spacing: 3) {
                HealthPill(health: monitor.health)
                Text("\(Format.bytes(s.avail)) free")
                    .font(.system(size: 21, weight: .bold, design: .rounded))
                    .monospacedDigit()
                    .lineLimit(1)
                Text("of \(Format.bytes(s.total)) on \(Disk.name)")
                    .font(.caption).foregroundStyle(.secondary)
                    .lineLimit(1)
            }
            Spacer(minLength: 0)
        }
    }

    private func forecast(_ s: Status) -> some View {
        VStack(alignment: .leading, spacing: 10) {
            VStack(alignment: .leading, spacing: 3) {
                Text(Headline.title(s)).font(.headline)
                Text(Headline.detail(s))
                    .font(.caption).foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
            if let series = monitor.detail?.series, series.count >= 2 {
                Sparkline(series: series,
                          color: monitor.health == .calm || monitor.health == .learning ? Theme.brand[0] : monitor.health.tint)
                    .frame(height: 46)
            }
        }
    }

    @ViewBuilder
    private var attention: some View {
        let rows = attentionRows
        if !rows.isEmpty {
            Divider()
            VStack(spacing: 2) {
                ForEach(rows.prefix(3), id: \.title) { r in
                    PanelRow(symbol: r.symbol, color: r.color, title: r.title) {
                        open { AppDelegate.show(.overview) }
                    }
                }
                if rows.count > 3 {
                    Text("and \(rows.count - 3) more")
                        .font(.caption).foregroundStyle(.secondary)
                        .padding(.leading, 46).padding(.vertical, 2)
                        .frame(maxWidth: .infinity, alignment: .leading)
                }
            }
            .padding(6)
        }
    }

    private struct Row {
        let symbol: String
        let color: Color
        let title: String
    }

    private var attentionRows: [Row] {
        var rows: [Row] = []
        if let p = monitor.watcherProblem { rows.append(Row(symbol: "pause.fill", color: .orange, title: p)) }
        if monitor.lacksFullDiskAccess {
            rows.append(Row(symbol: "lock.fill", color: .blue, title: "Needs Full Disk Access"))
        }
        for e in monitor.status?.incidents ?? [] {
            let i = e.incident
            let title = i.kind == .growth
                ? "\(URL(fileURLWithPath: i.subject).lastPathComponent) grows \(Format.bytes(i.perDay)) a day"
                : "\(i.subject) rewrites \(Format.bytes(i.perDay)) a day"
            rows.append(Row(symbol: i.kind == .growth ? "flame.fill" : "arrow.triangle.2.circlepath",
                            color: .red, title: title))
        }
        for t in monitor.undone {
            rows.append(Row(symbol: "arrow.uturn.backward", color: .purple, title: "Undone: \(t.title)"))
        }
        return rows
    }

    private var footer: some View {
        HStack(spacing: 8) {
            Button {
                open { AppDelegate.show(Navigator.shared.pane) }
            } label: {
                Text("Open MyDiskIsFull").frame(maxWidth: .infinity)
            }
            .buttonStyle(.borderedProminent)
            .controlSize(.large)
            .keyboardShortcut(.defaultAction)

            Menu {
                ForEach(Pane.allCases) { p in
                    Button(p.title) { open { AppDelegate.show(p) } }
                }
                Divider()
                Button("Settings…") { open { showSettings() } }
                    .keyboardShortcut(",", modifiers: .command)
                Button("About MyDiskIsFull") { open { AppDelegate.open("about") } }
                #if !APP_STORE
                UpdateItem(updates: monitor.updates)
                #endif
                Divider()
                Button("Quit MyDiskIsFull") { NSApp.terminate(nil) }
                    .keyboardShortcut("q", modifiers: .command)
            } label: {
                Image(systemName: "ellipsis.circle")
                    .font(.system(size: 17))
            }
            .menuStyle(.borderlessButton)
            .menuIndicator(.hidden)
            .fixedSize()
            .help("More")
        }
    }

    /// The panel goes, as a menu would, when what it opens comes forward.
    private func open(_ action: () -> Void) {
        let panel = NSApp.keyWindow
        action()
        if let panel, panel is NSPanel || String(describing: type(of: panel)).contains("MenuBarExtra") {
            panel.close()
        }
    }

    /// Settings would open behind whatever is in front: activate, then
    /// bring it forward.
    private func showSettings() {
        openSettings()
        NSApp.activate(ignoringOtherApps: true)
        DispatchQueue.main.async {
            NSApp.windows.first { $0.identifier?.rawValue == "com_apple_SwiftUI_Settings_window" }?
                .makeKeyAndOrderFront(nil)
        }
    }
}

/// A row that highlights under the pointer, like a menu item.
private struct PanelRow: View {
    let symbol: String
    let color: Color
    let title: String
    let action: () -> Void
    @State private var hovering = false

    var body: some View {
        Button(action: action) {
            HStack(spacing: 10) {
                IconTile(symbol: symbol, color: color, size: 24)
                Text(title).font(.callout).lineLimit(1).truncationMode(.middle)
                Spacer(minLength: 4)
                Image(systemName: "chevron.right")
                    .font(.caption.weight(.semibold))
                    .foregroundStyle(.tertiary)
            }
            .padding(.horizontal, 10)
            .padding(.vertical, 6)
            .contentShape(Rectangle())
            .background(hovering ? Color.primary.opacity(0.08) : .clear,
                        in: RoundedRectangle(cornerRadius: 7, style: .continuous))
        }
        .buttonStyle(.plain)
        .onHover { hovering = $0 }
    }
}

#if !APP_STORE
struct UpdateItem: View {
    @ObservedObject var updates: Updater

    var body: some View {
        Button("Check for Updates…") { updates.checkForUpdates() }
            .disabled(!updates.canCheckForUpdates)
    }
}
#endif

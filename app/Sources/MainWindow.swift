// The main window: a sidebar with the disk at a glance and the watcher's
// state, and four pages. The menu bar keeps only a glance and a way here.

import SwiftUI

enum Pane: String, CaseIterable, Identifiable {
    case overview, trends, map, fixes

    var id: String { rawValue }

    var title: String {
        switch self {
        case .overview: "Overview"
        case .trends: "Trends"
        case .map: "Disk Map"
        case .fixes: "Fixes"
        }
    }

    var symbol: String {
        switch self {
        case .overview: "gauge.with.dots.needle.67percent"
        case .trends: "chart.xyaxis.line"
        case .map: "chart.pie.fill"
        case .fixes: "wand.and.stars"
        }
    }

    var color: Color {
        switch self {
        case .overview: .blue
        case .trends: .purple
        case .map: .orange
        case .fixes: .green
        }
    }

    /// ⌘1 to ⌘4.
    var key: KeyEquivalent {
        KeyEquivalent(Character("\((Pane.allCases.firstIndex(of: self) ?? 0) + 1)"))
    }
}

/// Which page shows, so the menu bar, notifications and the menus can
/// open the window on the right one.
@MainActor
final class Navigator: ObservableObject {
    static let shared = Navigator()
    @Published var pane: Pane = .overview
}

struct MainWindow: View {
    @ObservedObject var monitor: Monitor
    @ObservedObject private var nav = Navigator.shared

    var body: some View {
        NavigationSplitView {
            Sidebar(monitor: monitor, selection: $nav.pane)
                .navigationSplitViewColumnWidth(min: 210, ideal: 230, max: 300)
        } detail: {
            Group {
                switch nav.pane {
                case .overview: OverviewPage(monitor: monitor)
                case .trends: TrendsPage(monitor: monitor)
                case .map: MapPage()
                case .fixes: FixesPage(monitor: monitor)
                }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .background(Theme.canvas)
            .navigationTitle(nav.pane.title)
        }
        .frame(minWidth: 900, minHeight: 620)
        .onAppear { monitor.viewed() }
        // The timer's refresh brings a new sample: the charts follow it.
        .task(id: monitor.lastRefresh) { await monitor.loadDetail() }
    }
}

private struct Sidebar: View {
    @ObservedObject var monitor: Monitor
    @Binding var selection: Pane

    var body: some View {
        List(selection: Binding(get: { selection }, set: { if let p = $0 { selection = p } })) {
            Section {
                ForEach(Pane.allCases) { p in
                    Label {
                        Text(p.title)
                    } icon: {
                        IconTile(symbol: p.symbol, color: p.color, size: 22)
                    }
                    .badge(badge(p))
                    .tag(p)
                }
            } header: {
                DiskSummary(monitor: monitor)
            }
        }
        .listStyle(.sidebar)
        .safeAreaInset(edge: .bottom, spacing: 0) {
            WatcherFooter(monitor: monitor)
        }
    }

    private func badge(_ p: Pane) -> Int {
        p == .overview ? monitor.attentionCount : 0
    }
}

/// The disk at the top of the sidebar: its name, a bar, and what's free.
private struct DiskSummary: View {
    @ObservedObject var monitor: Monitor

    var body: some View {
        VStack(alignment: .leading, spacing: 7) {
            HStack(spacing: 8) {
                Image(systemName: "internaldrive.fill")
                    .font(.title3)
                    .foregroundStyle(.secondary)
                Text(Disk.name)
                    .font(.headline)
                    .foregroundStyle(.primary)
                Spacer()
            }
            if let s = monitor.status {
                CapacityBar(used: Double(s.total - s.avail) / Double(max(s.total, 1)),
                            colors: monitor.health.ring)
                Text("\(Format.bytes(s.avail)) free of \(Format.bytes(s.total))")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .monospacedDigit()
            }
        }
        .textCase(nil)
        .padding(.horizontal, 4)
        .padding(.top, 4)
        .padding(.bottom, 12)
    }
}

/// Whether the watcher is watching, and a way to start it when it isn't.
private struct WatcherFooter: View {
    @ObservedObject var monitor: Monitor

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Divider()
            HStack(spacing: 9) {
                Circle()
                    .fill(problem == nil ? Color.green : Color.orange)
                    .frame(width: 8, height: 8)
                    .shadow(color: (problem == nil ? Color.green : Color.orange).opacity(0.6), radius: 3)
                VStack(alignment: .leading, spacing: 1) {
                    Text(problem ?? "Watching")
                        .font(.callout.weight(.medium))
                        .lineLimit(2)
                    if problem == nil, let last = monitor.status?.history.last {
                        TimelineView(.periodic(from: .now, by: 60)) { ctx in
                            Text("Last sample \(Format.ago(last, now: ctx.date))")
                                .font(.caption).foregroundStyle(.secondary)
                        }
                    }
                }
                Spacer(minLength: 0)
            }
            if problem != nil {
                if monitor.watcher.status == .requiresApproval {
                    Button("Open Login Items…") { monitor.watcher.openLoginItems() }
                        .controlSize(.small)
                } else {
                    Button("Start Watching") {
                        Task { await monitor.watcher.start(); await monitor.refresh() }
                    }
                    .controlSize(.small)
                    .buttonStyle(.borderedProminent)
                }
            }
        }
        .padding(.horizontal, 16)
        .padding(.bottom, 14)
    }

    private var problem: String? { monitor.watcherProblem }
}

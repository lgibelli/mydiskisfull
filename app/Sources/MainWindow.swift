// The main window, laid out as Apple's own apps are: a sidebar of five
// pages, with how much is free and whether the watcher is watching at its
// foot; each page's title in the toolbar; the pages grouped lists, and
// lists of files tables.
//
//   Overview    how the disk is doing, what needs attention, what to do
//   Storage     what takes the space, by category or by folder
//   Old Files   what nothing has opened or changed in a long time
//   History     free space over time, and what writes to the disk
//   Prevention  clean-ups and settings that keep it from filling again
//
// The menu bar keeps only a glance and a way here.

import SwiftUI

enum Pane: String, CaseIterable, Identifiable {
    case overview, storage, oldFiles, history, timeMachine, prevention

    var id: String { rawValue }

    /// The pages this edition has. The App Store one leaves out Prevention
    /// and Time Machine: the sandbox rules out all they do (tweaks,
    /// clean-ups, reading Time Machine's settings, marking folders), and a
    /// page that only points at the full version reads as a placeholder.
    static let shown: [Pane] = Edition.appStore
        ? allCases.filter { $0 != .prevention && $0 != .timeMachine } : allCases

    var title: String {
        switch self {
        case .overview: "Overview"
        case .storage: "Storage"
        case .oldFiles: "Old Files"
        case .history: "History"
        case .timeMachine: "Time Machine"
        case .prevention: "Prevention"
        }
    }

    var symbol: String {
        switch self {
        case .overview: "gauge.with.dots.needle.67percent"
        case .storage: "internaldrive"
        case .oldFiles: "clock.arrow.circlepath"
        case .history: "chart.xyaxis.line"
        case .timeMachine: "externaldrive.badge.timemachine"
        case .prevention: "checkmark.shield"
        }
    }

    /// ⌘1 to ⌘6 (⌘4 in the App Store edition).
    var key: KeyEquivalent {
        KeyEquivalent(Character("\((Pane.shown.firstIndex(of: self) ?? 0) + 1)"))
    }
}

/// Which page shows, so the menu bar, notifications and the menus can
/// open the window on the right one.
@MainActor
final class Navigator: ObservableObject {
    static let shared = Navigator()
    @Published var pane: Pane = .overview
    /// The categories opened on Storage, from the first.
    @Published var storagePath: [String] = []
    /// Settings, a sheet of the main window, and the page it opens on.
    @Published var settingsOpen = false
    var settingsPage: SettingsPage?

    func openSettings(_ page: SettingsPage? = nil) {
        settingsPage = page
        settingsOpen = true
    }

    /// Storage, open on a category.
    func show(kind: String) {
        storagePath = [kind]
        pane = .storage
    }
}

struct MainWindow: View {
    @ObservedObject var monitor: Monitor
    @ObservedObject private var nav = Navigator.shared

    var body: some View {
        NavigationSplitView {
            Sidebar(monitor: monitor, selection: $nav.pane)
                .navigationSplitViewColumnWidth(min: 190, ideal: 210, max: 280)
        } detail: {
            Group {
                switch nav.pane {
                case .overview: OverviewPage(monitor: monitor)
                case .storage: StoragePage(monitor: monitor)
                case .oldFiles: OldFilesPage(monitor: monitor)
                case .history: HistoryPage(monitor: monitor)
                case .timeMachine:
                    #if APP_STORE
                    EmptyView()
                    #else
                    TimeMachinePage(monitor: monitor)
                    #endif
                case .prevention: PreventionPage(monitor: monitor)
                }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
        .frame(minWidth: 820, minHeight: 560)
        .sheet(isPresented: $nav.settingsOpen) {
            SettingsView(monitor: monitor, page: nav.settingsPage)
        }
        .onAppear { monitor.viewed() }
        // While the window is on screen, free space follows what you delete:
        // `mdif status` reads it from the disk each time, in milliseconds.
        // Every 5 seconds while someone's looking (for two minutes after the
        // window came forward or free space moved), else once a minute; and
        // not at all while it's minimised, covered or on another Space. Each
        // read redraws the page, so a window left open all day polling every
        // 5 seconds cost more than the watcher.
        .task {
            var moved = Date.distantPast
            var read = Date()
            while !Task.isCancelled {
                try? await Task.sleep(for: .seconds(5))
                let now = Date()
                let looking = now.timeIntervalSince(max(moved, monitor.lookedAt)) < 120
                guard Self.onScreen, looking || now.timeIntervalSince(read) >= 60 else { continue }
                let before = monitor.status?.avail
                await monitor.refresh()
                read = Date()
                if let before, let after = monitor.status?.avail, abs(after - before) >= 10_000_000 { moved = read }
            }
        }
        // A new sample from the watcher (every 5 minutes): the charts follow
        // it. Not every read: the series is the whole history.
        .task(id: monitor.status?.history.last) { await monitor.loadDetail() }
    }

    /// A window of the app that someone could be looking at: shown, not
    /// minimised, at least partly uncovered, on the current Space.
    static var onScreen: Bool {
        NSApp.windows.contains { w in
            w.isVisible && w.occlusionState.contains(.visible) && w.styleMask.contains(.titled) && !(w is NSPanel)
        }
    }
}

private struct Sidebar: View {
    @ObservedObject var monitor: Monitor
    @Binding var selection: Pane

    var body: some View {
        List(selection: Binding(get: { selection }, set: { if let p = $0 { selection = p } })) {
            ForEach(Pane.shown) { p in
                Label(p.title, systemImage: p.symbol)
                    .badge(p == .overview ? monitor.attentionCount : 0)
                    .tag(p)
            }
        }
        .listStyle(.sidebar)
        .safeAreaInset(edge: .bottom, spacing: 0) {
            SidebarFooter(monitor: monitor)
        }
    }
}

/// At the foot of the sidebar, on every page: how much is free, and whether
/// the watcher is watching, with a way to start it when it isn't.
private struct SidebarFooter: View {
    @ObservedObject var monitor: Monitor

    var body: some View {
        VStack(alignment: .leading, spacing: 7) {
            HStack(spacing: 6) {
                Text(Disk.name).font(.caption.weight(.semibold))
                Spacer(minLength: 4)
                EditionBadge()
            }
            if let s = monitor.status {
                CapacityBar(used: Double(s.total - s.avail) / Double(max(s.total, 1)),
                            colors: [monitor.health.barTint], height: 5)
                Text("\(Format.bytes(s.avail)) available of \(Format.bytes(s.total))")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .monospacedDigit()
            }
            HStack(spacing: 6) {
                Circle()
                    .fill(problem == nil ? Color.green : Color.orange)
                    .frame(width: 7, height: 7)
                Group {
                    if let problem {
                        Text(problem).lineLimit(2)
                    } else if let last = monitor.status?.history.last {
                        TimelineView(.periodic(from: .now, by: 60)) { ctx in
                            Text("Watching · \(Format.ago(last, now: ctx.date))")
                        }
                    } else {
                        Text("Watching")
                    }
                }
                .font(.caption)
                .foregroundStyle(.secondary)
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
                }
            }
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .overlay(alignment: .top) { Divider() }
    }

    private var problem: String? { monitor.watcherProblem }
}

/// A page's toolbar button that measures again, or a spinner while it does.
struct MeasureButton: View {
    let measuring: Bool
    var help = "Measure the whole disk again"
    let action: () -> Void

    var body: some View {
        if measuring {
            ProgressView().controlSize(.small)
                .help("Measuring…")
        } else {
            Button(action: action) {
                Label("Measure Again", systemImage: "arrow.clockwise")
            }
            .help(help)
        }
    }
}

extension View {
    /// Toolbar items at the trailing edge, where a Mac app puts them. With
    /// the window's title shown they go there by themselves; the spacer
    /// (macOS 26) keeps them there when a page's controls sit in the middle.
    @ViewBuilder
    func trailingToolbar<Items: View>(@ViewBuilder _ items: () -> Items) -> some View {
        let content = items()
        if #available(macOS 26.0, *) {
            toolbar {
                ToolbarSpacer(.flexible)
                ToolbarItemGroup { content }
            }
        } else {
            toolbar { ToolbarItemGroup { content } }
        }
    }
}

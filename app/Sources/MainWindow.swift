// The main window: a sidebar with the disk at a glance and the watcher's
// state, and the pages: the overview (what needs attention, and the top
// few things to do from each page), where the space goes, cruft, how it
// changes, what writes, the map, and what keeps it from filling again.
// The menu bar keeps only a glance and a way here.

import SwiftUI

enum Pane: String, CaseIterable, Identifiable {
    case overview, trends, wear, map, space, cruft, prevent

    var id: String { rawValue }

    /// The sidebar's groups: watching the disk, and making room on it.
    /// The overview stands above both.
    enum Group: String, CaseIterable {
        case monitor = "Monitor", clean = "Clean"

        var panes: [Pane] { Pane.shown.filter { $0.group == self } }
    }

    /// The pages this edition has. The App Store one leaves out Disk Wear and
    /// Prevent: the sandbox rules out all they do (which apps write, tweaks,
    /// clean-ups), and a page that only points at the full version reads as a
    /// placeholder.
    static let shown: [Pane] = Edition.appStore ? allCases.filter { $0 != .wear && $0 != .prevent } : allCases

    var group: Group? {
        switch self {
        case .overview: nil
        case .trends, .wear, .map: .monitor
        case .space, .cruft, .prevent: .clean
        }
    }

    var title: String {
        switch self {
        case .overview: "Overview"
        case .space: "Space"
        case .trends: "Disk Usage"
        case .wear: "Disk Wear"
        case .cruft: "Cruft"
        case .map: "Disk Map"
        case .prevent: "Prevent"
        }
    }

    var symbol: String {
        switch self {
        case .overview: "gauge.with.dots.needle.67percent"
        case .space: "internaldrive.fill"
        case .trends: "chart.xyaxis.line"
        case .wear: "flame.fill"
        case .cruft: "archivebox.fill"
        case .map: "chart.pie.fill"
        case .prevent: "shield.lefthalf.filled"
        }
    }

    var color: Color {
        switch self {
        case .overview: .blue
        case .space: .teal
        case .trends: .purple
        case .wear: .red
        case .cruft: .brown
        case .map: .orange
        case .prevent: .green
        }
    }

    /// ⌘1 to ⌘7 (⌘5 in the App Store edition).
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
    /// Settings, a sheet of the main window, and the page it opens on.
    @Published var settingsOpen = false
    var settingsPage: SettingsPage?

    func openSettings(_ page: SettingsPage? = nil) {
        settingsPage = page
        settingsOpen = true
    }
}

struct MainWindow: View {
    @ObservedObject var monitor: Monitor
    @ObservedObject private var nav = Navigator.shared

    var body: some View {
        // The sidebar holds the disk's state on every page: it stays.
        NavigationSplitView(columnVisibility: .constant(.all)) {
            Sidebar(monitor: monitor, selection: $nav.pane)
                .toolbar(removing: .sidebarToggle)
                .navigationSplitViewColumnWidth(min: 230, ideal: 250, max: 320)
        } detail: {
            Group {
                switch nav.pane {
                case .overview: OverviewPage(monitor: monitor)
                case .space: SpacePage(monitor: monitor)
                case .trends: TrendsPage(monitor: monitor)
                case .wear: WearPage(monitor: monitor)
                case .cruft: CruftPage(monitor: monitor)
                case .map: MapPage()
                case .prevent: PreventPage(monitor: monitor)
                }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .background(Theme.canvas)
            .navigationTitle(nav.pane.title)
            // On the page's side of the window, not over the sidebar.
            .windowActions(measuring: monitor.spaceMeasuring,
                           refresh: { monitor.refreshAll() },
                           settings: { nav.openSettings() })
        }
        .frame(minWidth: 900, minHeight: 620)
        .sheet(isPresented: $nav.settingsOpen) {
            SettingsView(monitor: monitor, page: nav.settingsPage)
        }
        .onAppear { monitor.viewed() }
        // While the window is open, free space follows what you delete:
        // `mdif status` reads it from the disk each time, in milliseconds.
        .task {
            while !Task.isCancelled {
                try? await Task.sleep(for: .seconds(5))
                await monitor.refresh()
            }
        }
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
                row(.overview)
            } header: {
                DiskSummary(monitor: monitor)
            }
            ForEach(Pane.Group.allCases, id: \.self) { g in
                Section(g.rawValue) {
                    ForEach(g.panes) { row($0) }
                }
            }
        }
        .listStyle(.sidebar)
        .safeAreaInset(edge: .bottom, spacing: 0) {
            WatcherFooter(monitor: monitor)
        }
    }

    private func row(_ p: Pane) -> some View {
        Label {
            Text(p.title)
        } icon: {
            IconTile(symbol: p.symbol, color: p.color, size: 22)
        }
        .badge(badge(p))
        .tag(p)
    }

    private func badge(_ p: Pane) -> Int {
        p == .overview ? monitor.attentionCount : 0
    }
}

/// Refresh and Settings, in the window's toolbar.
private struct WindowActions: ToolbarContent {
    let measuring: Bool
    let refresh: () -> Void
    let settings: () -> Void

    var body: some ToolbarContent {
        ToolbarItem {
            if measuring {
                ProgressView().controlSize(.small)
                    .help("Measuring the whole disk…")
            } else {
                Button(action: refresh) {
                    Label("Refresh", systemImage: "arrow.clockwise")
                }
                .help("Measure everything again (⌘R)")
            }
        }
        ToolbarItem {
            Button(action: settings) {
                Label("Settings", systemImage: "gearshape")
            }
            .help("Settings (⌘,)")
        }
    }
}

private extension View {
    /// The window's actions at the toolbar's trailing edge, where a Mac app's
    /// Refresh goes. The window hides its title (each page shows its own),
    /// and without a title nothing pushes them there but a flexible spacer,
    /// which macOS 26 has; earlier versions put them where SwiftUI does. The
    /// check is out here, in a view builder, so ToolbarSpacer's type never
    /// reaches a toolbar on a macOS without it.
    @ViewBuilder
    func windowActions(measuring: Bool, refresh: @escaping () -> Void,
                       settings: @escaping () -> Void) -> some View {
        let actions = WindowActions(measuring: measuring, refresh: refresh, settings: settings)
        if #available(macOS 26.0, *) {
            toolbar {
                ToolbarSpacer(.flexible)
                actions
            }
        } else {
            toolbar { actions }
        }
    }
}

/// The disk at the top of the sidebar, on every page: how full it is, where
/// it's heading in a sentence, how fast, what's purgeable and in swap, and
/// the Trash with a button to empty it.
private struct DiskSummary: View {
    @ObservedObject var monitor: Monitor

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack(spacing: 8) {
                Image(systemName: "internaldrive.fill")
                    .foregroundStyle(.secondary)
                Text(Disk.name)
                    .font(.headline)
                    .foregroundStyle(.primary)
                Spacer()
                EditionBadge()
            }
            if let s = monitor.status {
                let used = Double(s.total - s.avail) / Double(max(s.total, 1))
                CapacityRing(used: used, colors: monitor.health.ring, lineWidth: 12) {
                    VStack(spacing: 1) {
                        Text(Format.bytes(s.avail))
                            .font(.system(size: 22, weight: .bold, design: .rounded))
                            .monospacedDigit()
                            .contentTransition(.numericText())
                        Text("free of \(Format.bytes(s.total))")
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    }
                }
                .frame(width: 150, height: 150)
                .frame(maxWidth: .infinity)
                VStack(alignment: .leading, spacing: 5) {
                    HealthPill(health: monitor.health)
                    Text(Headline.title(s))
                        .font(.title3.weight(.bold))
                        .foregroundStyle(.primary)
                        .fixedSize(horizontal: false, vertical: true)
                    Text(Headline.detail(s))
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                }
                VStack(spacing: 4) {
                    row("Used", "\(Format.bytes(s.total - s.avail)) · \(Int((used * 100).rounded()))%")
                    let pace = Headline.pace(s)
                    row(pace.label, pace.value)
                    // Counted as used, freed without a clean-up; the (i) on
                    // the first says why deleting doesn't always free space.
                    if let space = monitor.space {
                        let parts = space.byItself.filter { $0.bytes >= 500_000_000 }
                        ForEach(Array(parts.enumerated()), id: \.element.what) { i, p in
                            row(SpaceText.label(p.what), Format.bytes(p.bytes),
                                help: "\(SpaceText.label(p.what)): \(p.how).",
                                info: i == 0 ? space.byItself : nil)
                        }
                    }
                }
                if let trash = monitor.space?.trash, trash >= 100_000_000 {
                    TrashRow(bytes: trash, monitor: monitor)
                }
            }
        }
        .textCase(nil)
        .padding(.horizontal, 4)
        .padding(.top, 4)
        .padding(.bottom, 12)
    }

    private func row(_ label: String, _ value: String, help: String? = nil,
                     info: [SpaceReport.ComesBack]? = nil) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: 4) {
            Text(label).foregroundStyle(.secondary)
            if let info {
                FreeingInfo(parts: info)
            }
            Spacer(minLength: 8)
            Text(value).fontWeight(.semibold).foregroundStyle(.primary).monospacedDigit()
        }
        .font(.caption)
        .help(help ?? "")
    }
}

/// Why deleting doesn't always free space at once, behind an (i): what
/// APFS and macOS keep, and when each comes back.
private struct FreeingInfo: View {
    let parts: [SpaceReport.ComesBack]
    @State private var shown = false

    var body: some View {
        Button {
            shown.toggle()
        } label: {
            Image(systemName: "info.circle")
        }
        .buttonStyle(.borderless)
        .foregroundStyle(.secondary)
        .help("Why deleted files don't always free space at once")
        .popover(isPresented: $shown, arrowEdge: .trailing) {
            VStack(alignment: .leading, spacing: 10) {
                Text("Why deleting doesn't always free space at once")
                    .font(.headline)
                if !parts.isEmpty {
                    VStack(alignment: .leading, spacing: 3) {
                        ForEach(parts, id: \.what) { p in
                            Text("\(SpaceText.label(p.what)): \(Format.bytes(p.bytes)), \(p.how).")
                                .font(.callout.weight(.medium))
                        }
                    }
                }
                ForEach(Self.reasons, id: \.0) { r in
                    VStack(alignment: .leading, spacing: 2) {
                        Text(r.0).font(.callout.weight(.semibold))
                        Text(r.1).font(.callout).foregroundStyle(.secondary)
                            .fixedSize(horizontal: false, vertical: true)
                    }
                }
            }
            .padding(16)
            .frame(width: 380)
        }
    }

    static let reasons: [(String, String)] = [
        ("The Trash",
         "Deleting in Finder moves a file to the Trash. Its space comes back when the Trash is emptied."),
        ("Snapshots",
         "A local Time Machine snapshot keeps every file that existed when it was taken. A file it holds frees nothing until the snapshot goes, within a day, or when macOS needs the room."),
        ("Purgeable space",
         "Caches, iCloud copies and snapshots macOS can remove whenever something needs room. Finder counts them as available; the disk counts them as used until macOS removes them."),
        ("Files still open",
         "A file an app still has open keeps its space after it's deleted, until the app closes it or quits."),
        ("Swap",
         "Memory macOS moved to the disk. Its files are removed when the Mac restarts."),
        ("Copies that share space",
         "On APFS a copy can share its blocks with the original. Deleting one of them frees only what isn't shared."),
    ]
}

/// The Trash takes its space until it's emptied: how much, and a button
/// that empties it (`mdif clean ~/.Trash`), asked first.
private struct TrashRow: View {
    let bytes: Int64
    @ObservedObject var monitor: Monitor

    var body: some View {
        let path = NSHomeDirectory() + "/.Trash"
        let busy = monitor.cleaning.contains(path)
        HStack(spacing: 8) {
            Image(systemName: "trash").foregroundStyle(.orange)
            Text("\(Format.bytes(bytes)) in the Trash")
                .font(.caption.weight(.semibold))
                .foregroundStyle(.primary)
                .monospacedDigit()
            Spacer(minLength: 4)
            let action = CleanAction(
                kind: "empty-trash", label: "Empty Trash", paths: [path],
                confirm: "The \(Format.bytes(bytes)) in the Trash is erased for good. You can't undo this.",
                done: nil)
            #if APP_STORE
            // The App Store edition may only read: Finder empties it.
            if let c = action.shellCommand {
                Button("Copy Command") { monitor.copy(c) }
                    .buttonStyle(.link)
                    .font(.caption)
                    .help("\(c)\nOr choose Finder → Empty Trash.")
            }
            #else
            if busy {
                ProgressView().controlSize(.mini)
            } else {
                Button("Empty") { monitor.clean(action) }
                    .controlSize(.small)
            }
            #endif
        }
        .padding(8)
        .background(Color.orange.opacity(0.1), in: RoundedRectangle(cornerRadius: 8, style: .continuous))
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

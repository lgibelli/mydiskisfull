// Time Machine: where it backs up and how it's doing, what its local
// snapshots hold and why, what changed since the last backup (what the next
// one sends), the folders a build or an install makes again that are still
// backed up (keep them out, now and by themselves), and what its settings
// leave out. From `mdif timemachine`. Not in the App Store edition: the
// sandbox can neither read Time Machine's settings nor mark folders.

import AppKit
import SwiftUI

/// Sentences about Time Machine, for its page and Needs Attention.
enum TimeMachineText {
    /// "today at 10:22", "yesterday at 23:03", "Oct 27 at 23:03".
    static func when(_ ts: Int64) -> String {
        let d = Date(timeIntervalSince1970: TimeInterval(ts))
        let time = d.formatted(date: .omitted, time: .shortened)
        let cal = Calendar.current
        if cal.isDateInToday(d) { return "today at \(time)" }
        if cal.isDateInYesterday(d) { return "yesterday at \(time)" }
        return "\(d.formatted(.dateTime.month(.abbreviated).day())) at \(time)"
    }

    /// The same, at the start of a line: "Today at 10:22".
    static func whenTitle(_ ts: Int64) -> String {
        let w = when(ts)
        return w.prefix(1).uppercased() + w.dropFirst()
    }

    static func every(_ interval: Int64) -> String {
        switch interval {
        case ...3_600: "Every hour"
        case 86_400: "Every day"
        case 604_800: "Every week"
        default: "Every \(interval / 3_600) hours"
        }
    }

    static func backlog(_ b: TimeMachineReport.Backlog) -> String {
        let since = b.since.map { "since \(when($0))" } ?? "yet"
        let tries = b.failed == 0 ? "" : b.failed == 1 ? ": 1 try since didn't finish" : ": \(b.failed) tries since didn't finish"
        let held = b.held > 0 ? " (up to \(Format.bytes(b.held)) now)" : ""
        return "Time Machine hasn't finished a backup \(since)\(tries). Until one does, it keeps the snapshot it started from, so everything deleted or rewritten since stays on this disk\(held)."
    }

    static func what(_ kind: String) -> String {
        switch kind {
        case "dependencies": "Downloaded packages"
        case "cache": "Download cache"
        default: "Build output"
        }
    }

    /// What Finder calls it: "Final Cut Pro" for Final Cut Pro.app.
    static func name(_ path: String) -> String {
        let n = (path as NSString).lastPathComponent
        return n.hasSuffix(".app") ? String(n.dropLast(4)) : n
    }

    /// Where it is, short: an agent's worktree is "Worktree of <repository>",
    /// what comes with macOS "macOS", anything else the folder it's in.
    static func place(_ path: String) -> String {
        let parent = (path as NSString).deletingLastPathComponent
        if (parent as NSString).lastPathComponent == "worktrees",
           let repo = (parent as NSString).pathComponents.dropLast().last(where: { !$0.hasPrefix(".") && $0 != "/" }) {
            return "Worktree of \(repo)"
        }
        if path.hasPrefix("/System/") { return "macOS" }
        return Format.path(parent)
    }

    /// `path` from inside `folder`: "apps/web/dist".
    static func inside(_ folder: String, _ path: String) -> String {
        path.hasPrefix(folder + "/") ? String(path.dropFirst(folder.count + 1)) : name(path)
    }

    /// System Settings, at Time Machine.
    static func openSettings() {
        if let url = URL(string: "x-apple.systempreferences:com.apple.Time-Machine-Settings.extension") {
            NSWorkspace.shared.open(url)
        }
    }

    @MainActor static var icon: NSImage {
        NSWorkspace.shared.icon(forFile: "/System/Applications/Time Machine.app")
    }
}

#if !APP_STORE
struct TimeMachinePage: View {
    @ObservedObject var monitor: Monitor

    var body: some View {
        Form {
            if let r = monitor.timeMachine {
                if r.setUp {
                    BackupsSection(r: r, monitor: monitor)
                    SnapshotsSection(r: r, monitor: monitor)
                    if !r.changes.isEmpty { ChangesSection(r: r) }
                    RebuildableSection(r: r, monitor: monitor)
                    if !r.skipped.isEmpty { SkippedSection(r: r) }
                } else {
                    Section {
                        ContentUnavailableView {
                            Label("Time Machine isn't set up", systemImage: "externaldrive.badge.timemachine")
                        } description: {
                            Text("Choose a backup disk in System Settings, and this page shows what goes into the backups, and what its snapshots keep on this disk.")
                        } actions: {
                            Button("Time Machine Settings…") { TimeMachineText.openSettings() }
                        }
                    }
                }
            } else {
                HStack(spacing: 8) {
                    ProgressView().controlSize(.small)
                    Text("Asking Time Machine…").foregroundStyle(.secondary)
                }
            }
        }
        .formStyle(.grouped)
        .navigationTitle("Time Machine")
        .navigationSubtitle(subtitle)
        .trailingToolbar {
            MeasureButton(measuring: monitor.timeMachineLoading, help: "Look again") {
                Task { await monitor.loadTimeMachine() }
            }
        }
        .onAppear { monitor.viewed() }
        // The folders take a few seconds: a quick look is shown first.
        .task {
            if monitor.timeMachine?.rebuildable.isEmpty ?? true { await monitor.loadTimeMachine(quick: true) }
            await monitor.loadTimeMachine()
            await monitor.loadTidies()
        }
    }

    private var subtitle: String {
        guard let r = monitor.timeMachine, r.setUp else { return "" }
        if r.running != nil { return "Backing up…" }
        return r.lastBackup.map { "Last backup \(Format.ago($0))" } ?? "No backup yet"
    }
}

private struct BackupsSection: View {
    let r: TimeMachineReport
    @ObservedObject var monitor: Monitor

    var body: some View {
        Section {
            if let b = r.backlog {
                AttentionRow(symbol: "exclamationmark.triangle.fill", color: .orange,
                             title: "Backups keep failing", text: TimeMachineText.backlog(b)) {
                    EmptyView()
                }
            }
            HStack(spacing: 12) {
                Image(nsImage: TimeMachineText.icon).resizable().frame(width: 36, height: 36)
                VStack(alignment: .leading, spacing: 2) {
                    Text(r.destinations.map(\.name).joined(separator: ", ")).font(.headline)
                    Text(place).foregroundStyle(.secondary)
                }
                Spacer()
                if let run = r.running {
                    HStack(spacing: 8) {
                        if let done = run.done {
                            ProgressView(value: done).frame(width: 80)
                            Text("\(Int((done * 100).rounded()))%").monospacedDigit().foregroundStyle(.secondary)
                        } else {
                            ProgressView().controlSize(.small)
                            Text("Backing up…").foregroundStyle(.secondary)
                        }
                    }
                } else {
                    Button("Back Up Now") { monitor.backUpNow() }
                        .disabled(monitor.backupStarting)
                }
            }
            .padding(.vertical, 2)
            LabeledContent("Last backup") {
                Text(r.lastBackup.map { TimeMachineText.when($0) } ?? "None yet")
            }
            LabeledContent {
                HStack(spacing: 8) {
                    Text(r.auto ? TimeMachineText.every(r.interval) : "Only when you ask")
                    Button("Change…") { TimeMachineText.openSettings() }
                }
            } label: {
                Text("How often")
                if r.interval > 3_600 {
                    Text("More often doesn't make the snapshots smaller: macOS keeps each for a day. It helps when backups get interrupted: each carries an hour of changes instead of a day's, so it finishes sooner.")
                }
            }
            ForEach(r.destinations.indices, id: \.self) { i in
                let d = r.destinations[i]
                if let used = d.used {
                    LabeledContent(r.destinations.count == 1 ? "Backup disk" : d.name) {
                        Text(room(used: used, d)).monospacedDigit()
                    }
                }
            }
            if !r.tries.isEmpty {
                DisclosureGroup("Latest tries") {
                    ForEach(r.tries) { t in
                        HStack(spacing: 8) {
                            Image(systemName: t.finished == nil ? "xmark.circle.fill" : "checkmark.circle.fill")
                                .foregroundStyle(t.finished == nil ? Color.orange : Color.green)
                            Text(TimeMachineText.whenTitle(t.started))
                            Spacer()
                            Text(t.finished.map { $0 == t.started ? "Finished" : "Finished \(TimeMachineText.when($0))" }
                                 ?? "Didn't finish")
                                .foregroundStyle(.secondary)
                        }
                    }
                }
            }
        } header: {
            Text("Backups")
        }
    }

    private var place: String {
        let servers = r.destinations.compactMap(\.server)
        return servers.isEmpty ? "A backup disk" : "On \(servers.joined(separator: ", ")), over the network"
    }

    private func room(used: Int64, _ d: TimeMachineReport.Destination) -> String {
        if let q = d.quota { return "\(Format.bytes(used)) used of \(Format.bytes(q))" }
        if let a = d.available { return "\(Format.bytes(used)) used, \(Format.bytes(a)) free" }
        return "\(Format.bytes(used)) used"
    }
}

private struct SnapshotsSection: View {
    let r: TimeMachineReport
    @ObservedObject var monitor: Monitor

    var body: some View {
        Section {
            if r.snapshots.isEmpty {
                Text("None now: what you delete frees its space at once.").foregroundStyle(.secondary)
            }
            ForEach(r.snapshots) { s in
                LabeledContent {
                    if s.reference { Pill(text: "Next backup starts here", color: .blue) }
                } label: {
                    Text(TimeMachineText.whenTitle(s.taken))
                    Text(s.reference ? "Kept until a backup finishes" : "macOS deletes it a day after it was taken")
                }
            }
            if !r.snapshots.isEmpty {
                HStack {
                    VStack(alignment: .leading, spacing: 2) {
                        Text(r.held > 0 ? "Up to \(Format.bytes(r.held)) held" : "What they hold")
                        Text("What macOS counts as purgeable: it deletes them by itself when it needs the room.")
                            .font(.subheadline).foregroundStyle(.secondary)
                    }
                    Spacer()
                    if let a = r.delete { CleanButton(action: a, monitor: monitor) }
                }
            }
        } header: {
            Text("Local Snapshots")
        } footer: {
            Text("Before each backup, Time Machine takes a snapshot of the whole disk, on the disk itself. It keeps every file as it was then, excluded folders too: deleting or rewriting a file frees nothing while a snapshot holds the old copy.")
                .foregroundStyle(.secondary)
        }
    }
}

private struct ChangesSection: View {
    let r: TimeMachineReport

    var body: some View {
        Section {
            ForEach(r.changes) { c in
                let name = c.name ?? TimeMachineText.name(c.path)
                HStack(spacing: 10) {
                    FileIcon(path: c.path)
                    VStack(alignment: .leading, spacing: 1) {
                        Text(name)
                        // Named from what's known: the folder's own name stays in sight.
                        let place = c.name == nil ? TimeMachineText.place(c.path) : Format.path(c.path)
                        Text("\(place) · \(state(c))")
                            .font(.subheadline).foregroundStyle(.secondary)
                            .lineLimit(1).truncationMode(.middle)
                    }
                    Spacer(minLength: 10)
                    Text((c.bytes >= 0 ? "+" : "−") + Format.bytes(abs(c.bytes)))
                        .monospacedDigit()
                        .foregroundStyle(c.bytes >= 0 ? Color.primary : Color.secondary)
                    if let known = c.known {
                        KnownButton(title: name, known: known)
                    } else if c.unexplained != nil {
                        #if !APP_STORE
                        // What goes into a backup isn't the folder's size.
                        AskButton(title: name, path: c.path, bytes: nil)
                        #endif
                    }
                    PathMenu(path: c.path)
                }
                .help(Format.path(c.path))
            }
        } header: {
            Text("Since the Last Backup")
        } footer: {
            Text("What the watcher saw grow and shrink since \(r.lastBackup.map { TimeMachineText.when($0) } ?? "then"). What grew goes into the next backup; what was deleted stays in the snapshots until then.")
                .foregroundStyle(.secondary)
        }
    }

    private func state(_ c: TimeMachineReport.Change) -> String {
        if c.backedUp == false { return "Not backed up" }
        if c.bytes < 0 { return "Deleted: held by the snapshots until the next backup" }
        if c.keptOut > 0 {
            return "Goes into the next backup, except \(c.keptOut) folder\(c.keptOut == 1 ? "" : "s") kept out"
        }
        return "Goes into the next backup"
    }
}

/// One project's build folders: a repository with its worktrees, so a
/// project with forty `node_modules` and `dist` is one row. Or a tool's
/// caches, in its folder in the home folder.
private struct Project: Identifiable {
    let path: String
    let folders: [TimeMachineReport.Folder]
    var id: String { path }
    var bytes: Int64 { folders.reduce(0) { $0 + $1.bytes } }
    var worktrees: Int { Set(folders.map(\.checkout)).subtracting([path]).count }
    var tool: Bool { folders.allSatisfy { $0.what == "cache" } }

    /// "3 projects", "2 tools", "3 projects and 2 tools".
    static func count(_ all: [Project]) -> String {
        let tools = all.filter(\.tool).count
        let projects = all.count - tools
        let p = projects == 1 ? "1 project" : "\(projects) projects"
        let t = tools == 1 ? "1 tool" : "\(tools) tools"
        return tools == 0 ? p : projects == 0 ? t : "\(p) and \(t)"
    }

    /// Biggest first; those not measured (kept out) by how many.
    static func of(_ folders: [TimeMachineReport.Folder]) -> [Project] {
        Dictionary(grouping: folders, by: \.project)
            .map { Project(path: $0.key, folders: $0.value.sorted { ($0.bytes, $1.path) > ($1.bytes, $0.path) }) }
            .sorted { ($0.bytes, $0.folders.count, $1.path) > ($1.bytes, $1.folders.count, $0.path) }
    }

    /// "48 folders: node_modules, dist, build · 14 worktrees", or for one
    /// folder, where it is and what: ".venv · Downloaded packages". A
    /// tool's caches by where they are: "2 caches: cache, plugins/cache".
    var detail: String {
        if folders.count == 1, let f = folders.first {
            let what = TimeMachineText.what(f.what)
            return f.path == path ? what : "\(TimeMachineText.inside(path, f.path)) · \(what)"
        }
        var seen: [String: (bytes: Int64, count: Int)] = [:]
        for f in folders {
            let n = tool ? TimeMachineText.inside(path, f.path) : TimeMachineText.name(f.path)
            seen[n, default: (0, 0)].bytes += f.bytes
            seen[n, default: (0, 0)].count += 1
        }
        let names = seen.sorted { ($0.value.bytes, $0.value.count, $1.key) > ($1.value.bytes, $1.value.count, $0.key) }.map(\.key)
        let kinds = names.prefix(3).joined(separator: ", ") + (names.count > 3 ? "…" : "")
        let w = worktrees
        return "\(folders.count) \(tool ? "caches" : "folders"): \(kinds)" + (w == 0 ? "" : " · \(w) worktree\(w == 1 ? "" : "s")")
    }
}

private struct RebuildableSection: View {
    let r: TimeMachineReport
    @ObservedObject var monitor: Monitor
    @State private var all = false
    private static let first = 6

    var body: some View {
        let backed = Project.of(r.rebuildable.filter(\.backedUp))
        let out = Project.of(r.rebuildable.filter { !$0.backedUp })
        let count = backed.reduce(0) { $0 + $1.folders.count }
        let outCount = r.rebuildable.count - count
        let total = backed.reduce(0) { $0 + $1.bytes }
        let shown = all ? backed : Array(backed.prefix(Self.first))
        Section {
            if r.rebuildable.isEmpty {
                HStack(spacing: 8) {
                    if monitor.timeMachineLoading { ProgressView().controlSize(.small) }
                    Text(monitor.timeMachineLoading ? "Looking through your code and tools' folders…" : "No build output, packages or caches in your code and tools' folders.")
                        .foregroundStyle(.secondary)
                }
            } else if !backed.isEmpty {
                HStack {
                    VStack(alignment: .leading, spacing: 2) {
                        Text("\(Format.bytes(total)) in \(count) folder\(count == 1 ? "" : "s")")
                            .font(.headline).monospacedDigit()
                        Text("In \(Project.count(backed)): backed up, though \(backed.allSatisfy(\.tool) ? "each tool downloads what it needs again" : backed.contains(where: \.tool) ? "a build, an install or the tool makes them again" : "their next build or install makes them again").")
                            .font(.subheadline).foregroundStyle(.secondary)
                    }
                    Spacer()
                    Button("Keep All Out") { monitor.setBackedUp(backed.flatMap { $0.folders.map(\.path) }, false) }
                        .disabled(!monitor.backupChanging.isDisjoint(with: backed.flatMap { $0.folders.map(\.path) }))
                }
                ForEach(shown) { ProjectRow(project: $0, monitor: monitor) }
                if backed.count > Self.first {
                    Button(all ? "Show Fewer" : "Show \(backed.count - Self.first) More") {
                        withAnimation(.snappy) { all.toggle() }
                    }
                    .buttonStyle(.link)
                }
            }
            if let t = monitor.tidies?.first(where: { $0.id == "backups" }) {
                Toggle(isOn: Binding(get: { t.on }, set: { monitor.setTidy(t.id, on: $0) })) {
                    Text("Keep new ones out by themselves")
                    Text("MyDiskIsFull looks for new ones every 6 hours, agents' worktrees too, and leaves in what you back up. Nothing is deleted.")
                }
                .toggleStyle(.switch)
                .disabled(monitor.tidyBusy.contains(t.id))
            }
            if !out.isEmpty {
                DisclosureGroup {
                    ForEach(out) { ProjectRow(project: $0, monitor: monitor) }
                } label: {
                    let projects = Project.count(out)
                    if backed.isEmpty {
                        Label {
                            Text("All \(outCount) kept out, in \(projects)")
                        } icon: {
                            Image(systemName: "checkmark.circle.fill").foregroundStyle(.green)
                        }
                    } else {
                        Text("\(outCount) kept out already, in \(projects)")
                    }
                }
            }
        } header: {
            Text("Build Folders and Caches in the Backups")
        } footer: {
            Text("Keeping them out means less to send over the network and less room on the backup disk. It doesn't make the local snapshots smaller: those cover the whole disk.")
                .foregroundStyle(.secondary)
        }
    }
}

/// A project: its name, how much and what, and one button for all its
/// folders; more than one, and they're listed inside.
private struct ProjectRow: View {
    let project: Project
    @ObservedObject var monitor: Monitor

    var body: some View {
        if project.folders.count == 1 {
            row
        } else {
            DisclosureGroup {
                ForEach(project.folders) { FolderRow(folder: $0, monitor: monitor) }
            } label: {
                row
            }
        }
    }

    private var row: some View {
        let p = project
        let paths = p.folders.map(\.path)
        let backedUp = p.folders.first?.backedUp ?? false
        return HStack(spacing: 10) {
            FileIcon(path: p.path)
            VStack(alignment: .leading, spacing: 1) {
                Text(TimeMachineText.name(p.path))
                Text(p.detail).font(.subheadline).foregroundStyle(.secondary)
                    .lineLimit(1).truncationMode(.middle)
            }
            Spacer(minLength: 10)
            if backedUp {
                Text(Format.bytes(p.bytes)).monospacedDigit()
            }
            if !monitor.backupChanging.isDisjoint(with: paths) {
                ProgressView().controlSize(.small)
            } else {
                Button(backedUp ? "Keep Out" : "Back Up") { monitor.setBackedUp(paths, !backedUp) }
            }
            PathMenu(path: p.path)
        }
        .help(Format.path(p.path))
        .contextMenu { PathMenuItems(path: p.path) }
    }
}

/// One folder inside a project: where in it, and in which worktree.
private struct FolderRow: View {
    let folder: TimeMachineReport.Folder
    @ObservedObject var monitor: Monitor

    var body: some View {
        let f = folder
        let worktree = f.checkout != f.project ? "Worktree \(TimeMachineText.name(f.checkout)) · " : ""
        HStack(spacing: 10) {
            FileIcon(path: f.path, size: 20)
            VStack(alignment: .leading, spacing: 1) {
                Text(TimeMachineText.inside(f.checkout, f.path)).lineLimit(1).truncationMode(.middle)
                Text(worktree + TimeMachineText.what(f.what))
                    .font(.subheadline).foregroundStyle(.secondary)
                    .lineLimit(1).truncationMode(.middle)
            }
            Spacer(minLength: 10)
            if f.backedUp {
                Text(Format.bytes(f.bytes)).monospacedDigit()
            }
            if monitor.backupChanging.contains(f.path) {
                ProgressView().controlSize(.small)
            } else {
                Button(f.backedUp ? "Keep Out" : "Back Up") { monitor.setBackedUp([f.path], !f.backedUp) }
            }
            PathMenu(path: f.path)
        }
        .help(Format.path(f.path))
        .contextMenu { PathMenuItems(path: f.path) }
    }
}

private struct SkippedSection: View {
    let r: TimeMachineReport

    var body: some View {
        Section {
            DisclosureGroup("\(r.skipped.count) item\(r.skipped.count == 1 ? "" : "s")") {
                ForEach(r.skipped, id: \.self) { p in
                    HStack(spacing: 10) {
                        FileIcon(path: p)
                        VStack(alignment: .leading, spacing: 1) {
                            Text(TimeMachineText.name(p))
                            Text(TimeMachineText.place(p) + (missingApp(p) ? " · Not installed" : ""))
                                .font(.subheadline).foregroundStyle(.secondary)
                                .lineLimit(1).truncationMode(.middle)
                        }
                        Spacer(minLength: 10)
                        PathMenu(path: p)
                    }
                    .help(Format.path(p))
                }
            }
            Button("Time Machine Settings…") { TimeMachineText.openSettings() }
        } header: {
            Text("Left Out in Time Machine's Settings")
        } footer: {
            Text("macOS also leaves out caches, the Trash and its own files.")
                .foregroundStyle(.secondary)
        }
    }

    /// An app left out by name that isn't on this Mac (only apps are
    /// looked for: a folder in Desktop would ask for permission).
    private func missingApp(_ path: String) -> Bool {
        path.hasSuffix(".app") && !FileManager.default.fileExists(atPath: path)
    }
}
#endif

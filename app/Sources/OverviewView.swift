// The overview, as System Settings shows storage: the disk and its bar,
// how it's doing in a sentence, what needs attention, one list of what to
// do (biggest first, each with its button), and what changed lately.

import AppKit
import SwiftUI

struct OverviewPage: View {
    @ObservedObject var monitor: Monitor

    var body: some View {
        Group {
            if let s = monitor.status {
                Form {
                    Section {
                        DiskSummary(status: s, monitor: monitor)
                        StatusRow(status: s, health: monitor.health)
                    }
                    AttentionSection(monitor: monitor)
                    RecommendationsSection(monitor: monitor)
                    RecentSection(monitor: monitor)
                }
                .formStyle(.grouped)
            } else if let e = monitor.statusError {
                ContentUnavailableView {
                    Label("Can't read the watcher's history", systemImage: "exclamationmark.triangle")
                } description: {
                    Text(e)
                }
            } else {
                ProgressView()
            }
        }
        .navigationTitle("Overview")
        .navigationSubtitle(subtitle)
        .task {
            await monitor.loadSpace()
            #if !APP_STORE
            await monitor.loadTidies()
            #endif
        }
    }

    private var subtitle: String {
        guard let first = monitor.status?.history.first else { return "" }
        return "Watched since \(Format.day(first))"
    }
}

// MARK: - The disk

/// The disk's icon, name and what's available, and its bar by category: a
/// click on a part opens it on Storage.
private struct DiskSummary: View {
    let status: Status
    @ObservedObject var monitor: Monitor

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            HStack(spacing: 12) {
                Image(nsImage: Disk.icon)
                    .resizable()
                    .frame(width: 44, height: 44)
                VStack(alignment: .leading, spacing: 2) {
                    Text(Disk.name).font(.headline)
                    Text("\(Format.bytes(status.avail)) available of \(Format.bytes(status.total))")
                        .foregroundStyle(.secondary)
                        .monospacedDigit()
                        .contentTransition(.numericText())
                }
                Spacer()
            }
            if let r = monitor.space {
                RailBar(stops: RailBar.Stop.of(r, limit: 6), total: max(r.volume.total, 1)) {
                    Navigator.shared.show(kind: $0)
                }
            } else {
                CapacityBar(used: Double(status.total - status.avail) / Double(max(status.total, 1)),
                            colors: [monitor.health.barTint], height: 8)
            }
        }
        .padding(.vertical, 4)
    }
}

/// How the disk is doing: where it's heading, in a sentence and its reason.
private struct StatusRow: View {
    let status: Status
    let health: Health

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 10) {
            Image(systemName: health.symbol)
                .foregroundStyle(health.tint)
                .font(.title3)
            VStack(alignment: .leading, spacing: 3) {
                Text(Headline.title(status)).font(.headline)
                Text(Headline.detail(status))
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
        .padding(.vertical, 2)
    }
}

// MARK: - What needs attention

private struct AttentionSection: View {
    @ObservedObject var monitor: Monitor

    var body: some View {
        if monitor.attentionCount > 0 {
            Section("Needs Attention") {
                if let p = monitor.watcherProblem {
                    AttentionRow(symbol: "pause.fill", color: .orange, title: p, text: monitor.watcher.caption) {
                        if monitor.watcher.status == .requiresApproval {
                            Button("Open Login Items…") { monitor.watcher.openLoginItems() }
                        } else {
                            Button("Start Watching") {
                                Task { await monitor.watcher.start(); await monitor.refresh() }
                            }
                        }
                    }
                }
                if monitor.lacksFullDiskAccess {
                    #if APP_STORE
                    AttentionRow(symbol: "lock.fill", color: .blue, title: "Let \(Edition.name) see your disk",
                                 text: "Until then it watches free space only. To find which folders grow, it needs to see \(DiskAccess.startupDisk). It reads names, sizes and dates, never what's in a file.") {
                        Button("Choose Disk…") { monitor.chooseDisk() }
                    }
                    #else
                    AttentionRow(symbol: "lock.fill", color: .blue, title: "Let MyDiskIsFull see the whole disk",
                                 text: "Without Full Disk Access, what grows in other apps' folders goes unseen. It only ever reads sizes, never what's in a file.") {
                        Button("Open Privacy Settings…") { Privacy.openSettings() }
                    }
                    #endif
                }
                ForEach(monitor.status?.incidents ?? []) { e in
                    AttentionRow(symbol: e.incident.kind == .growth ? "flame.fill" : "arrow.triangle.2.circlepath",
                                 color: .red, title: e.title,
                                 text: "\(e.sentence) It \(e.incident.kind == .growth ? "grows" : "writes") \(Format.bytes(e.incident.perDay)) a day.") {
                        EmptyView()
                    } fix: {
                        FixActions(fix: e.fix, line: e.fixLine,
                                   action: monitor.suggestion(for: e)?.action, monitor: monitor)
                    }
                }
                #if !APP_STORE
                ForEach(monitor.undone) { t in
                    AttentionRow(symbol: "arrow.uturn.backward", color: .purple, title: "Undone since: \(t.title)",
                                 text: "Something changed the setting back, probably an update.") {
                        Button("Apply Again…") { monitor.apply(t.id) }
                    }
                }
                #endif
            }
        }
    }
}

/// A row of Needs Attention: what, why, and its button at the end; a fix
/// that needs more room (a command, steps) goes under it.
struct AttentionRow<Button: View, Fix: View>: View {
    let symbol: String
    let color: Color
    let title: String
    let text: String
    @ViewBuilder var button: Button
    @ViewBuilder var fix: Fix

    var body: some View {
        HStack(alignment: .top, spacing: 12) {
            IconTile(symbol: symbol, color: color, size: 28)
            VStack(alignment: .leading, spacing: 6) {
                Text(title).font(.body.weight(.medium))
                Text(Format.inline(text))
                    .font(.subheadline)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
                fix
            }
            Spacer(minLength: 12)
            button
        }
        .padding(.vertical, 3)
    }
}

extension AttentionRow where Fix == EmptyView {
    init(symbol: String, color: Color, title: String, text: String, @ViewBuilder button: () -> Button) {
        self.init(symbol: symbol, color: color, title: title, text: text, button: button) { EmptyView() }
    }
}

// MARK: - What to do

/// One thing to do: what, why in a line, how much it frees, and its button.
struct Recommendation: Identifiable {
    enum Act {
        /// The app does it (`mdif clean`).
        case clean(CleanAction)
        /// A tweak, reviewed before it's applied.
        case tweak(String)
        /// A clean-up the watcher does once a day, turned on.
        case tidy(String)
        /// A command to paste into Terminal: the app never runs it.
        case copy(String)
        /// A page with the rest.
        case show(Pane, String)
        /// Storage, open on a category.
        case kind(String, String)
        case none
    }

    let id: String
    let symbol: String
    let tint: Color
    let title: String
    var detail: String?
    var bytes: Int64?
    var act: Act = .none
    /// What it's about, shown in Finder and kept from the ⋯ menu.
    var path: String?
    var keepable = false
    /// The whole story, behind the (i): the cause, the steps, the command.
    var more: String?
    var command: String?
    var sizes: [Suggestion.SizePoint]?
}

private struct RecommendationsSection: View {
    @ObservedObject var monitor: Monitor
    @AppStorage("cruft.minBytes") private var minBytes: Double = 100_000_000
    @AppStorage("cruft.minDays") private var minDays: Int = 365
    @State private var all = false
    private static let first = 5

    var body: some View {
        let list = Recommendations.all(monitor, minBytes: Int64(minBytes), minDays: minDays)
        Section {
            if !monitor.spaceReadable {
                Text("To see what can be freed, let \(Edition.name) see the whole disk.")
                    .foregroundStyle(.secondary)
            } else if monitor.space == nil {
                HStack(spacing: 10) {
                    ProgressView().controlSize(.small)
                    Text("Measuring the whole disk: about a minute the first time.")
                        .foregroundStyle(.secondary)
                }
            } else if list.isEmpty {
                Text("Nothing big to clean up.").foregroundStyle(.secondary)
            }
            ForEach(all ? list : Array(list.prefix(Self.first))) { r in
                RecommendationRow(item: r, monitor: monitor)
            }
            if list.count > Self.first {
                Button(all ? "Show Fewer" : "Show All \(list.count)") {
                    withAnimation(.snappy) { all.toggle() }
                }
                .buttonStyle(.link)
            }
        } header: {
            Text("Recommendations")
        }
    }
}

struct RecommendationRow: View {
    let item: Recommendation
    @ObservedObject var monitor: Monitor

    var body: some View {
        let r = item
        HStack(spacing: 12) {
            IconTile(symbol: r.symbol, color: r.tint, size: 28)
            VStack(alignment: .leading, spacing: 2) {
                Text(r.title).lineLimit(1).truncationMode(.middle)
                if let d = r.detail {
                    Text(Format.inline(d))
                        .font(.subheadline)
                        .foregroundStyle(.secondary)
                        .lineLimit(2)
                }
            }
            Spacer(minLength: 12)
            if let b = r.bytes {
                Text(Format.bytes(b))
                    .foregroundStyle(.secondary)
                    .monospacedDigit()
                    .fixedSize()
            }
            ActButton(act: r.act, monitor: monitor)
            if r.more != nil || r.command != nil || (r.sizes?.count ?? 0) >= 2 {
                MoreButton(item: r, monitor: monitor)
            }
            if let p = r.path {
                PathMenu(path: p, keep: r.keepable ? { monitor.keep([p]) } : nil)
            }
        }
        .padding(.vertical, 2)
    }
}

/// A recommendation's button.
private struct ActButton: View {
    let act: Recommendation.Act
    @ObservedObject var monitor: Monitor

    var body: some View {
        switch act {
        case .clean(let a):
            #if APP_STORE
            if let c = a.shellCommand {
                Button("Copy Command") { monitor.copy(c) }
                    .help("\(c)\n\nPaste it into Terminal. The full version does it for you.")
            }
            #else
            CleanButton(action: a, monitor: monitor)
            #endif
        case .tweak(let id):
            #if APP_STORE
            FullVersionNote(missing: .tweaks, title: "Turn On", compact: true)
            #else
            Button("Turn On…") { monitor.apply(id) }
                .disabled(monitor.tweaksChanging[id] != nil)
            #endif
        case .tidy(let id):
            // The App Store edition has no clean-ups to turn on.
            #if !APP_STORE
            Button("Turn On") { monitor.setTidy(id, on: true) }
                .disabled(monitor.tidyBusy.contains(id))
            #else
            let _ = id
            #endif
        case .copy(let c):
            Button("Copy Command") { monitor.copy(c) }
                .help(c)
        case .show(let pane, let label):
            Button(label) { Navigator.shared.pane = pane }
        case .kind(let id, let label):
            Button(label) { Navigator.shared.show(kind: id) }
        case .none:
            EmptyView()
        }
    }
}

/// The (i) at the end of a row: the whole story, the steps, the command.
private struct MoreButton: View {
    let item: Recommendation
    @ObservedObject var monitor: Monitor
    @State private var shown = false

    var body: some View {
        Button {
            shown.toggle()
        } label: {
            Image(systemName: "info.circle")
        }
        .buttonStyle(.borderless)
        .foregroundStyle(.secondary)
        .help("More about this")
        .popover(isPresented: $shown, arrowEdge: .bottom) {
            VStack(alignment: .leading, spacing: 10) {
                Text(item.title).font(.headline)
                if let more = item.more {
                    Text(Format.inline(more)).fixedSize(horizontal: false, vertical: true)
                }
                if let sizes = item.sizes, sizes.count >= 2 {
                    SizeBars(points: sizes, tint: item.tint)
                }
                if let c = item.command {
                    CommandBox(command: c) { monitor.copy(c) }
                }
            }
            .padding(16)
            .frame(width: 400)
        }
    }
}

/// Everything there is to do, biggest first: clean-ups of what's on Storage,
/// the rest of the suggestions, old files (as one, for Old Files), the
/// Trash, then tweaks and daily clean-ups not on. What runs away is under
/// Needs Attention instead.
@MainActor
enum Recommendations {
    static func all(_ m: Monitor, minBytes: Int64, minDays: Int) -> [Recommendation] {
        guard let r = m.space else { return [] }
        let forgotten = SpaceText.forgotten(r, min: minBytes, days: minDays, limit: .max)
        let skip = Set(forgotten.map(\.id))
        var out: [Recommendation] = []
        for k in r.kinds {
            let listed = k.items.filter { !$0.has("kept") && !skip.contains($0.id) }
            // Clean-ups with nothing more to say: three or more go as one
            // row, which opens the category.
            var plain: [Recommendation] = []
            for i in listed {
                // Inside one that is listed: it's counted there.
                let inside = listed.contains { o in
                    i.path.hasPrefix(o.path + "/") && (o.action != nil || m.suggestion(about: o) != nil)
                }
                if inside { continue }
                if let s = m.suggestion(about: i) {
                    guard !s.leave else { continue }
                    // About the item itself, or something in it (what it frees).
                    out.append(from(s, id: i.id, title: s.path == i.path ? i.name : s.headline,
                                    symbol: SpaceText.symbol(k.id), tint: SpaceText.color(k.id),
                                    bytes: s.frees.flatMap { $0 > 0 ? $0 : nil } ?? (i.frees > 0 ? i.frees : i.bytes),
                                    fallback: i.action))
                } else if let a = i.action {
                    plain.append(Recommendation(id: i.id, symbol: SpaceText.symbol(k.id), tint: SpaceText.color(k.id),
                                                title: i.name, detail: SpaceText.marks(i).first?.0 ?? i.note,
                                                bytes: i.frees > 0 ? i.frees : i.bytes, act: .clean(a),
                                                path: i.path, keepable: SpaceText.mayGo(i), more: i.note))
                } else if let about = i.about, let a = about.action {
                    out.append(Recommendation(id: i.id, symbol: SpaceText.symbol(k.id), tint: SpaceText.color(k.id),
                                              title: about.title, detail: i.name, bytes: about.bytes,
                                              act: .clean(a), path: about.path, more: about.how))
                }
            }
            plain = plain.filter { ($0.bytes ?? 0) >= minRow }
            if plain.count >= 3 {
                out.append(group(k, plain))
            } else {
                out += plain
            }
        }
        for s in m.unplacedSuggestions where !s.leave {
            out.append(from(s, id: s.cause, title: s.headline, symbol: s.symbol, tint: s.tint,
                            bytes: s.frees.flatMap { $0 > 0 ? $0 : nil } ?? s.space, fallback: nil))
        }
        // What nothing has opened in that long, as one: Old Files lists it.
        let span = Int64(minDays) * 86_400
        let files = (r.oldFiles ?? []).filter { !$0.kept && $0.bytes >= minBytes && $0.lastUsed <= r.measured - span }
        let old = forgotten.reduce(0) { $0 + $1.bytes } + files.reduce(0) { $0 + $1.bytes }
        if old >= 100_000_000 {
            let n = forgotten.count + files.count
            out.append(Recommendation(id: "old-files", symbol: Pane.oldFiles.symbol, tint: .brown,
                                      title: "Review old files",
                                      detail: "\(n) file\(n == 1 ? "" : "s") and folder\(n == 1 ? "" : "s") nothing has opened in \(OldFilesPage.age(minDays)) or more",
                                      bytes: old, act: .show(.oldFiles, "Review…")))
        }
        if r.trash >= 100_000_000 {
            let path = NSHomeDirectory() + "/.Trash"
            out.append(Recommendation(
                id: "trash", symbol: "trash.fill", tint: .gray, title: "Empty the Trash",
                detail: "What's in it takes its space until it's emptied.", bytes: r.trash,
                act: .clean(CleanAction(kind: "empty-trash", label: "Empty Trash", paths: [path],
                                        confirm: "The \(Format.bytes(r.trash)) in the Trash is erased for good. You can't undo this.",
                                        done: nil))))
        }
        out.sort { ($0.bytes ?? 0) > ($1.bytes ?? 0) }
        #if !APP_STORE
        let tweaks = (m.tweaks ?? []).filter { !$0.isApplied && !$0.isAbsent }
            .sorted { ($0.size ?? 0) > ($1.size ?? 0) }
        let placed = Set(out.compactMap { if case .tweak(let id) = $0.act { id } else { nil } })
        for t in tweaks where !placed.contains(t.id) {
            out.append(Recommendation(id: "tweak:\(t.id)", symbol: "wand.and.stars", tint: .green, title: t.title,
                                      detail: t.why, act: .tweak(t.id), more: "\(t.why)\n\nYou give up: \(t.cost)"))
        }
        for t in (m.tidies ?? []) where !t.on && t.available {
            out.append(Recommendation(id: "tidy:\(t.id)", symbol: "sparkles", tint: .green,
                                      title: "Clean up \(t.title.lowercased()) every day", detail: t.detail,
                                      act: .tidy(t.id)))
        }
        #endif
        return out
    }

    /// Smaller than this, a clean-up isn't worth a row.
    private static let minRow: Int64 = 100_000_000

    /// A category's clean-ups as one row: how many, the biggest by name.
    private static func group(_ k: SpaceReport.Kind, _ rows: [Recommendation]) -> Recommendation {
        let rows = rows.sorted { ($0.bytes ?? 0) > ($1.bytes ?? 0) }
        let (title, noun) = switch k.id {
        case "caches": ("Empty caches", "caches")
        case "build": ("Delete build output", "folders")
        default: ("Clean up \(k.title.lowercased())", "items")
        }
        let names = rows.prefix(3).map(\.title).joined(separator: ", ")
        return Recommendation(id: "kind:\(k.id)", symbol: SpaceText.symbol(k.id), tint: SpaceText.color(k.id),
                              title: title,
                              detail: "\(rows.count) \(noun): \(names)" + (rows.count > 3 ? " and \(rows.count - 3) more" : ""),
                              bytes: rows.reduce(0) { $0 + ($1.bytes ?? 0) }, act: .kind(k.id, "Review…"))
    }

    /// A suggestion as a row: its button, or the command to copy, or its
    /// first step; the rest behind (i).
    private static func from(_ s: Suggestion, id: String, title: String, symbol: String, tint: Color,
                             bytes: Int64?, fallback: CleanAction?) -> Recommendation {
        var r = Recommendation(id: id, symbol: symbol, tint: tint, title: title, bytes: bytes,
                               path: s.path, keepable: s.path != nil, more: s.cause, sizes: s.sizes)
        // How it grew, or what the cause says; the first step at least.
        r.detail = s.detail ?? (s.title != nil ? s.cause : nil) ?? s.firstStep
        if r.detail == title { r.detail = nil }
        switch s.fix {
        case .tweak(let t):
            r.act = s.action.map { .clean($0) } ?? .tweak(t)
        case .run(let command, let note):
            r.act = (s.action ?? fallback).map { .clean($0) } ?? .copy(command)
            r.more = [s.cause, note].joined(separator: "\n\n")
            r.command = command
        case .steps(let text):
            r.act = (s.action ?? fallback).map { .clean($0) } ?? .none
            r.more = [s.cause, text].joined(separator: "\n\n")
        case .leave(let text):
            r.more = [s.cause, text].joined(separator: "\n\n")
        }
        return r
    }
}

// MARK: - What changed lately

private struct RecentSection: View {
    @ObservedObject var monitor: Monitor

    var body: some View {
        let paces = monitor.status?.forecast?.paces
        let grew = (monitor.detail?.growth3d ?? monitor.detail?.growth24h ?? []).first { $0.bytes >= 100_000_000 }
        Section {
            if let day = paces?.day {
                LabeledContent("Last 24 hours") { Change(bytes: day) }
            }
            if let week = paces?.week {
                LabeledContent("Last 7 days") { Change(bytes: week * 7) }
            }
            if let g = grew {
                LabeledContent("Grew the most in \(monitor.detail?.growth3d == nil ? "24 hours" : "3 days")") {
                    HStack(spacing: 6) {
                        Text(URL(fileURLWithPath: g.owner).lastPathComponent)
                        Text("+\(Format.bytes(g.bytes))").monospacedDigit()
                    }
                    .help(Format.path(g.owner))
                }
            }
            #if !APP_STORE
            if let w = monitor.detail?.writes24h, !w.isEmpty {
                LabeledContent("Written in 24 hours") {
                    Text(Format.bytes(w.reduce(0) { $0 + $1.bytes })).monospacedDigit()
                }
            }
            #endif
        } header: {
            HStack {
                Text("Recent Changes")
                Spacer()
                Button("Show History") { Navigator.shared.pane = .history }
                    .buttonStyle(.link)
                    .font(.callout)
            }
        }
    }
}

/// "Filled 3.4 GB" in orange, "Freed 2.9 GB" in green, "Quiet".
struct Change: View {
    let bytes: Double

    var body: some View {
        Text(Format.change(bytes))
            .monospacedDigit()
            .foregroundStyle(abs(bytes) < 50_000_000 ? Color.secondary : bytes > 0 ? Color.orange : Color.green)
    }
}

// MARK: - Fixes

/// What to do about something that runs away: a tweak to apply, a command
/// to copy (never run), or what to change by hand.
struct FixActions: View {
    let fix: Fix
    let line: String
    var action: CleanAction? = nil
    @ObservedObject var monitor: Monitor

    var body: some View {
        switch fix {
        case .tweak(let id):
            let title = monitor.tweak(id)?.title ?? line
            #if APP_STORE
            FullVersionNote(missing: .tweaks, title: "Fix: \(title)", compact: true)
            #else
            if monitor.tweak(id)?.isApplied == true {
                Label("\(title): applied", systemImage: "checkmark.circle.fill")
                    .foregroundStyle(.green)
            } else {
                Button("\(title)…") { monitor.apply(id) }
            }
            #endif
        case .run(let command, let note):
            VStack(alignment: .leading, spacing: 8) {
                Text(Format.inline(note)).font(.subheadline).fixedSize(horizontal: false, vertical: true)
                if let action {
                    #if APP_STORE
                    LiteCommand(action: action, monitor: monitor)
                    #else
                    CleanButton(action: action, monitor: monitor)
                    #endif
                } else {
                    CommandBox(command: command) { monitor.copy(command) }
                }
            }
        case .steps(let text):
            VStack(alignment: .leading, spacing: 8) {
                Text(Format.inline(text)).font(.subheadline).fixedSize(horizontal: false, vertical: true)
                if let action {
                    #if APP_STORE
                    LiteCommand(action: action, monitor: monitor)
                    #else
                    CleanButton(action: action, monitor: monitor)
                    #endif
                }
            }
        case .leave(let text):
            Text("Leave it: " + text).font(.subheadline).fixedSize(horizontal: false, vertical: true)
        }
    }
}

#if !APP_STORE
/// A clean-up's button, done by the app (CleanFlow); while it runs, what
/// it's doing.
struct CleanButton: View {
    let action: CleanAction
    @ObservedObject var monitor: Monitor

    var body: some View {
        let busy = action.paths.first.map { monitor.cleaning.contains($0) } ?? false
        if let done = action.done {
            // Pressing it again would do nothing more until the watcher has
            // seen whether it worked.
            Label("Restarted at \(Format.time(done))", systemImage: "checkmark.circle")
                .foregroundStyle(.secondary)
                .help("Watching whether its writes stop.")
        } else if busy {
            HStack(spacing: 6) {
                ProgressView().controlSize(.small)
                Text(action.busy).foregroundStyle(.secondary)
            }
            .fixedSize()
        } else {
            Button(action.label) { monitor.clean(action) }
                .fixedSize()
        }
    }
}
#endif

#if APP_STORE
/// Where the full version has a clean-up button: that it's there, and the
/// same done by hand, to paste into Terminal.
struct LiteCommand: View {
    let action: CleanAction
    @ObservedObject var monitor: Monitor

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            FullVersionNote(missing: .cleanups, title: action.label, compact: true)
            if let c = action.shellCommand {
                Text("Or do it yourself: paste this into Terminal. It can't be undone.")
                    .font(.caption).foregroundStyle(.secondary)
                CommandBox(command: c) { monitor.copy(c) }
            }
        }
    }
}
#endif

/// A command to copy, in a code box: the app never runs it.
struct CommandBox: View {
    let command: String
    let copy: () -> Void
    @State private var copied = false

    var body: some View {
        HStack(spacing: 10) {
            Text(command)
                .font(.system(.callout, design: .monospaced))
                .textSelection(.enabled)
                .lineLimit(3)
                .frame(maxWidth: .infinity, alignment: .leading)
            Button {
                copy()
                withAnimation { copied = true }
                Task {
                    try? await Task.sleep(for: .seconds(1.5))
                    withAnimation { copied = false }
                }
            } label: {
                Label(copied ? "Copied" : "Copy", systemImage: copied ? "checkmark" : "doc.on.doc")
                    .frame(minWidth: 64)
            }
            .controlSize(.small)
        }
        .padding(.horizontal, 10)
        .padding(.vertical, 7)
        .background(Color.primary.opacity(0.05), in: RoundedRectangle(cornerRadius: 6, style: .continuous))
    }
}

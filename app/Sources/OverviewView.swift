// The overview: what needs attention, each with what to do; then the top
// three things to do from each page (what to free on Space, what's
// forgotten on Cruft, what grew, what writes, what to turn on), each with
// its button and a way to the page for the rest. How full the disk is and
// where it's heading is in the sidebar, on every page (MainWindow.swift).

import AppKit
import SwiftUI

struct OverviewPage: View {
    @ObservedObject var monitor: Monitor

    var body: some View {
        Page(title: "Overview", subtitle: subtitle) {
            if monitor.status != nil {
                AttentionSection(monitor: monitor)
                NextSteps(monitor: monitor)
            } else if let e = monitor.statusError {
                ContentUnavailableView {
                    Label("Can't read the watcher's history", systemImage: "exclamationmark.triangle")
                } description: {
                    Text(e)
                }
                .frame(minHeight: 360)
            } else {
                ProgressView().frame(maxWidth: .infinity, minHeight: 360)
            }
        }
    }

    private var subtitle: String {
        guard let s = monitor.status, let first = s.history.first else { return Disk.name }
        return "\(Disk.name) · watched since \(Format.when(first))"
    }
}

// MARK: - What needs attention

private struct AttentionSection: View {
    @ObservedObject var monitor: Monitor

    var body: some View {
        let s = monitor.status
        let count = monitor.attentionCount
        VStack(alignment: .leading, spacing: 12) {
            if count == 0 {
                AllClear()
            } else {
                HStack(spacing: 8) {
                    Text("Needs your attention").font(.title3.weight(.semibold))
                    Pill(text: "\(count)", color: monitor.health == .urgent ? .red : .orange)
                }
                if let p = monitor.watcherProblem {
                    AttentionCard(symbol: "pause.fill", color: .orange, title: p,
                                  text: monitor.watcher.caption) {
                        if monitor.watcher.status == .requiresApproval {
                            Button("Open Login Items…") { monitor.watcher.openLoginItems() }
                        } else {
                            Button("Start Watching") {
                                Task { await monitor.watcher.start(); await monitor.refresh() }
                            }
                            .buttonStyle(.borderedProminent)
                        }
                    }
                }
                if monitor.lacksFullDiskAccess {
                    #if APP_STORE
                    AttentionCard(symbol: "lock.fill", color: .blue,
                                  title: "Let \(Edition.name) see your disk",
                                  text: "Until then it watches free space only. To find which folders grow, and to map the disk, it needs to see \(DiskAccess.startupDisk): choose it in the dialog. It reads names, sizes and dates, never what's in a file.") {
                        Button("Choose Disk…") { monitor.chooseDisk() }
                            .buttonStyle(.borderedProminent)
                    }
                    #else
                    AttentionCard(symbol: "lock.fill", color: .blue,
                                  title: "Let MyDiskIsFull see the whole disk",
                                  text: "Without Full Disk Access, macOS keeps the watcher out of other apps' folders, so what grows there goes unseen. It only ever reads sizes, never what's in a file.") {
                        Button("Open Privacy Settings…") { Privacy.openSettings() }
                            .buttonStyle(.borderedProminent)
                    }
                    #endif
                }
                ForEach(s?.incidents ?? []) { e in
                    AttentionCard(symbol: e.incident.kind == .growth ? "flame.fill" : "arrow.triangle.2.circlepath",
                                  color: .red, title: title(e), text: e.sentence,
                                  badge: "\(Format.bytes(e.incident.perDay)) a day") {
                        FixActions(fix: e.fix, line: e.fixLine,
                                   action: monitor.suggestion(for: e)?.action, monitor: monitor)
                    }
                }
                #if !APP_STORE
                ForEach(monitor.undone) { t in
                    AttentionCard(symbol: "arrow.uturn.backward", color: .purple,
                                  title: "Undone since: \(t.title)",
                                  text: "Something changed the setting back, probably an update. Apply it again to keep the space from filling.") {
                        Button("Apply Again…") { monitor.apply(t.id) }
                            .buttonStyle(.borderedProminent)
                    }
                }
                #endif
            }
        }
    }

    private func title(_ e: Episode) -> String { e.title }
}

private struct AllClear: View {
    var body: some View {
        HStack(spacing: 16) {
            Image(systemName: "checkmark.seal.fill")
                .font(.system(size: 30))
                .foregroundStyle(.white, Color.green.gradient)
            VStack(alignment: .leading, spacing: 3) {
                Text("Nothing out of the ordinary").font(.headline)
                Text("No folder is running away and no app is churning the disk. \(Edition.name) keeps watching \(Edition.appStore ? "while it's open" : "in the background") and tells you when that changes.")
                    .font(.callout)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
            Spacer(minLength: 0)
        }
        .card(padding: 18, tint: .green)
    }
}

struct AttentionCard<Actions: View>: View {
    let symbol: String
    let color: Color
    let title: String
    let text: String
    var badge: String?
    @ViewBuilder let actions: Actions

    var body: some View {
        HStack(alignment: .top, spacing: 14) {
            IconTile(symbol: symbol, color: color, size: 34)
            VStack(alignment: .leading, spacing: 8) {
                HStack(alignment: .firstTextBaseline) {
                    Text(title).font(.headline)
                    Spacer(minLength: 8)
                    if let badge { Pill(text: badge, color: color, symbol: "arrow.up.right") }
                }
                Text(text)
                    .font(.callout)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
                actions
                    .padding(.top, 2)
            }
        }
        .card(padding: 16)
    }
}

// MARK: - The top three from each page

private struct NextSteps: View {
    @ObservedObject var monitor: Monitor
    @AppStorage("cruft.minBytes") private var minBytes: Double = 100_000_000
    @AppStorage("cruft.minDays") private var minDays: Int = 365

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            SectionHeader(title: "What to do next", subtitle: "The biggest from each page. See All for the rest.")
            if !monitor.spaceReadable {
                HubCard(pane: .space, summary: "Needs the whole disk") {
                    Text("To see where the space goes, let \(Edition.name) see the whole disk.")
                        .font(.callout).foregroundStyle(.secondary)
                        .frame(maxWidth: .infinity, alignment: .leading)
                }
            } else if let r = monitor.space {
                spaceCard(r)
                cruftCard(r)
            } else {
                HStack(spacing: 10) {
                    ProgressView().controlSize(.small)
                    Text("Measuring the whole disk: about a minute the first time. It reads names, sizes and dates, never what's in a file.")
                        .font(.callout).foregroundStyle(.secondary)
                }
                .card()
            }
            grewCard
            #if !APP_STORE
            wearCard
            preventCard
            #endif
        }
        .task {
            await monitor.loadSpace()
            #if !APP_STORE
            await monitor.loadTidies()
            #endif
        }
    }

    @ViewBuilder private func spaceCard(_ r: SpaceReport) -> some View {
        let top = spaceSteps(r).filter { ($0.bytes ?? 0) >= 100_000_000 }
            .sorted { ($0.bytes ?? 0) > ($1.bytes ?? 0) }.prefix(3)
        let waste = r.kinds.reduce(0) { $0 + $1.reclaimable }
        HubCard(pane: .space, summary: waste >= 1_000_000_000
                ? "\(Format.bytes(waste)) of \(Format.bytes(r.volume.used)) you may not need"
                : "\(Format.bytes(r.volume.used)) used, measured \(Format.ago(r.measured))") {
            if top.isEmpty {
                HubEmpty(text: "Nothing big to clean up.")
            }
            ForEach(Array(top.enumerated()), id: \.element.id) { n, step in
                if n > 0 { Divider() }
                HubRow(step: step, monitor: monitor)
            }
        }
    }

    /// What frees the most: items with a button or a suggestion, and the
    /// suggestions about no item, without what's forgotten (on Cruft).
    private func spaceSteps(_ r: SpaceReport) -> [ToDo] {
        let forgotten = Set(SpaceText.forgotten(r, min: Int64(minBytes), days: minDays, limit: .max).map(\.id))
        let items: [ToDo] = r.kinds.flatMap(\.items).compactMap { i in
            guard !i.has("kept"), !forgotten.contains(i.id) else { return nil }
            if let s = monitor.suggestion(about: i), !s.leave {
                // About the item itself, or something in it (what it frees).
                return s.path == i.path
                    ? ToDo(id: i.id, name: i.name, detail: s.headline, bytes: s.frees ?? (i.frees > 0 ? i.frees : i.bytes),
                           action: s.action ?? i.action)
                    : ToDo(id: i.id, name: s.headline, detail: s.folder, bytes: s.frees ?? s.space, action: s.action)
            }
            if let a = i.action {
                return ToDo(id: i.id, name: i.name, detail: SpaceText.marks(i).first?.0,
                            bytes: i.frees > 0 ? i.frees : i.bytes, action: a)
            }
            if let about = i.about, let a = about.action {
                return ToDo(id: i.id, name: about.title, detail: i.name, bytes: about.bytes, action: a)
            }
            return nil
        }
        return items + monitor.unplacedSuggestions.filter { !$0.leave }.map {
            ToDo(id: $0.cause, name: $0.headline, detail: $0.folder ?? $0.firstStep, bytes: $0.frees ?? $0.space,
                 action: $0.action)
        }
    }

    /// The biggest of what looks forgotten, folders and files, by the
    /// thresholds chosen on Cruft.
    @ViewBuilder private func cruftCard(_ r: SpaceReport) -> some View {
        let span = Int64(minDays) * 86_400
        let folders = SpaceText.forgotten(r, min: Int64(minBytes), days: minDays, limit: .max).map { i in
            // Its mark, and what it is: "Application" says little alone.
            ToDo(id: i.id, name: i.name,
                 detail: [SpaceText.marks(i).first { $0.0 != "Kept" }?.0, i.note].compactMap { $0 }
                    .joined(separator: " · "),
                 bytes: i.bytes, action: i.action, path: i.path)
        }
        let files = (r.oldFiles ?? [])
            .filter { !$0.kept && $0.bytes >= Int64(minBytes) && $0.lastUsed <= r.measured - span }
            .map { f in
                ToDo(id: f.id, name: f.name, detail: "Unused for \(Format.age(r.measured - f.lastUsed))",
                     bytes: f.bytes, action: f.action, path: f.path)
            }
        let all = folders + files
        if !all.isEmpty {
            let top = all.sorted { ($0.bytes ?? 0) > ($1.bytes ?? 0) }.prefix(3)
            let total = all.reduce(0) { $0 + ($1.bytes ?? 0) }
            HubCard(pane: .cruft, summary: "\(Format.bytes(total)) in \(folders.count) folder\(folders.count == 1 ? "" : "s") and \(files.count) file\(files.count == 1 ? "" : "s") unused for \(CruftPage.age(minDays)) or more") {
                ForEach(Array(top.enumerated()), id: \.element.id) { n, step in
                    if n > 0 { Divider() }
                    HubRow(step: step, monitor: monitor)
                }
            }
        }
    }

    @ViewBuilder private var grewCard: some View {
        // Three days: a download or an update from the day before yesterday
        // is still what filled the disk.
        let threeDays = monitor.detail?.growth3d
        let rows = (threeDays ?? monitor.detail?.growth24h ?? []).filter { $0.bytes >= 100_000_000 }.prefix(3)
        if !rows.isEmpty {
            HubCard(pane: .trends, summary: "Grew the most in the last \(threeDays == nil ? "24 hours" : "3 days")") {
                ForEach(Array(rows.enumerated()), id: \.offset) { n, g in
                    if n > 0 { Divider() }
                    let url = URL(fileURLWithPath: g.owner)
                    HubRow(step: ToDo(id: g.owner, name: url.lastPathComponent,
                                      detail: Format.path(url.deletingLastPathComponent().path),
                                      bytes: g.bytes, sign: "+", path: g.owner),
                           monitor: monitor)
                }
            }
        }
    }

    #if !APP_STORE
    @ViewBuilder private var wearCard: some View {
        let rows = (monitor.detail?.writes24h ?? []).filter { $0.bytes >= 1_000_000_000 }
        if !rows.isEmpty {
            let written = rows.reduce(0) { $0 + $1.bytes }
            HubCard(pane: .wear, summary: "\(Format.bytes(written)) written in the last 24 hours") {
                ForEach(Array(rows.prefix(3).enumerated()), id: \.offset) { n, w in
                    if n > 0 { Divider() }
                    HubRow(step: ToDo(id: w.name, name: w.name, bytes: w.bytes), monitor: monitor)
                }
            }
        }
    }

    /// Tweaks not applied, the biggest at stake first, then clean-ups the
    /// watcher could keep doing.
    @ViewBuilder private var preventCard: some View {
        let tweaks = (monitor.tweaks ?? []).filter { !$0.isApplied && !$0.isAbsent }
            .sorted { ($0.size ?? 0) > ($1.size ?? 0) }
        let tidies = (monitor.tidies ?? []).filter { !$0.on && $0.available }
        if !tweaks.isEmpty || !tidies.isEmpty {
            let off = tweaks.count + tidies.count
            HubCard(pane: .prevent, summary: "\(off) way\(off == 1 ? "" : "s") to keep it from filling, not on") {
                let rows: [(String, String, String, Bool)] =
                    tweaks.map { ($0.id, $0.title, $0.why, true) } + tidies.map { ($0.id, $0.title, $0.detail, false) }
                ForEach(Array(rows.prefix(3).enumerated()), id: \.element.0) { n, row in
                    if n > 0 { Divider() }
                    HStack(spacing: 10) {
                        VStack(alignment: .leading, spacing: 1) {
                            Text(row.1).font(.callout.weight(.medium)).lineLimit(1)
                            Text(row.2).font(.caption).foregroundStyle(.secondary).lineLimit(1)
                        }
                        Spacer(minLength: 8)
                        if row.3 {
                            Button("Turn On…") { monitor.apply(row.0) }
                                .controlSize(.small)
                                .disabled(monitor.tweaksChanging[row.0] != nil)
                        } else {
                            Button("Turn On") { monitor.setTidy(row.0, on: true) }
                                .controlSize(.small)
                                .disabled(monitor.tidyBusy.contains(row.0))
                        }
                    }
                    .padding(.vertical, 7)
                }
            }
        }
    }
    #endif
}

/// One thing to do in a hub card.
private struct ToDo: Identifiable {
    let id: String
    let name: String
    var detail: String?
    var bytes: Int64?
    /// "+" for what grew.
    var sign = ""
    var action: CleanAction?
    /// Shown in the Finder.
    var path: String?
}

/// A page's top few: its name, what it found, the rows, and a way there.
private struct HubCard<Rows: View>: View {
    let pane: Pane
    let summary: String
    @ViewBuilder let rows: Rows

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 12) {
                IconTile(symbol: pane.symbol, color: pane.color, size: 30)
                VStack(alignment: .leading, spacing: 2) {
                    Text(pane.title).font(.headline)
                    Text(summary).font(.caption).foregroundStyle(.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                }
                Spacer(minLength: 8)
                Button {
                    Navigator.shared.pane = pane
                } label: {
                    HStack(spacing: 3) {
                        Text("See All")
                        Image(systemName: "chevron.right").font(.caption.weight(.semibold))
                    }
                }
                .buttonStyle(.link)
            }
            VStack(spacing: 0) { rows }
                .padding(.top, 6)
                .padding(.leading, 42)
        }
        .card(padding: 14)
    }
}

private struct HubEmpty: View {
    let text: String

    var body: some View {
        Text(text).font(.callout).foregroundStyle(.secondary)
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(.vertical, 7)
    }
}

private struct HubRow: View {
    let step: ToDo
    @ObservedObject var monitor: Monitor

    var body: some View {
        HStack(spacing: 10) {
            VStack(alignment: .leading, spacing: 1) {
                Text(step.name).font(.callout.weight(.medium)).lineLimit(1).truncationMode(.middle)
                if let d = step.detail {
                    Text(Format.inline(d)).font(.caption).foregroundStyle(.secondary).lineLimit(1)
                }
            }
            Spacer(minLength: 8)
            #if !APP_STORE
            if let a = step.action {
                CleanButton(action: a, command: nil, monitor: monitor)
                    .controlSize(.small)
                    .fixedSize()
            }
            #endif
            if let b = step.bytes {
                Text(step.sign + Format.bytes(b))
                    .font(.system(.callout, design: .rounded).weight(.semibold))
                    .monospacedDigit()
                    .frame(minWidth: 70, alignment: .trailing)
            }
            if let p = step.path {
                Button {
                    NSWorkspace.shared.activateFileViewerSelecting([URL(fileURLWithPath: p)])
                } label: {
                    Image(systemName: "folder")
                }
                .buttonStyle(.borderless)
                .foregroundStyle(.secondary)
                .help("Show in Finder")
            }
        }
        .padding(.vertical, 7)
    }
}

/// What to do about a cause: a tweak to apply, a command to copy (never
/// run), or what to change by hand.
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
                    .font(.callout)
                    .foregroundStyle(.green)
            } else {
                Button {
                    monitor.apply(id)
                } label: {
                    Label("\(title)…", systemImage: "wand.and.stars")
                }
                .buttonStyle(.borderedProminent)
            }
            #endif
        case .run(let command, let note):
            VStack(alignment: .leading, spacing: 8) {
                Text(Format.inline(note)).font(.callout).fixedSize(horizontal: false, vertical: true)
                if let action {
                    #if APP_STORE
                    FullVersionNote(missing: .cleanups, title: action.label, compact: true)
                    CommandBox(command: command) { monitor.copy(command) }
                    #else
                    CleanButton(action: action, command: command, monitor: monitor)
                    #endif
                } else {
                    CommandBox(command: command) { monitor.copy(command) }
                }
            }
        case .steps(let text):
            VStack(alignment: .leading, spacing: 8) {
                Label {
                    Text(Format.inline(text)).fixedSize(horizontal: false, vertical: true)
                } icon: {
                    Image(systemName: "hand.point.right.fill").foregroundStyle(.blue)
                }
                .font(.callout)
                if let action {
                    #if APP_STORE
                    LiteCommand(action: action, monitor: monitor)
                    #else
                    CleanButton(action: action, command: nil, monitor: monitor)
                    #endif
                }
            }
        case .leave(let text):
            Label {
                Text("Leave it: " + text).fixedSize(horizontal: false, vertical: true)
            } icon: {
                Image(systemName: "leaf.fill").foregroundStyle(.teal)
            }
            .font(.callout)
        }
    }
}

#if !APP_STORE
/// A suggestion's clean-up, done by the app (CleanFlow); the command it
/// stands for can still be copied.
struct CleanButton: View {
    let action: CleanAction
    let command: String?
    @ObservedObject var monitor: Monitor

    var body: some View {
        let busy = action.paths.first.map { monitor.cleaning.contains($0) } ?? false
        HStack(spacing: 12) {
            if let done = action.done {
                // Pressing it again would do nothing more until the watcher
                // has seen whether it worked.
                Label("Restarted at \(Format.time(done))", systemImage: "checkmark.circle")
                    .foregroundStyle(.secondary)
                Text("Watching whether its writes stop.")
                    .font(.callout).foregroundStyle(.secondary)
            } else {
                Button {
                    monitor.clean(action)
                } label: {
                    Label(action.label, systemImage: action.symbol)
                }
                .buttonStyle(.borderedProminent)
                .disabled(busy)
            }
            if busy {
                ProgressView().controlSize(.small)
                Text(action.busy)
                    .font(.callout).foregroundStyle(.secondary)
            }
            Spacer()
            if let command {
                Button("Copy Command") { monitor.copy(command) }
                    .buttonStyle(.link)
                    .font(.callout)
                    .help(command)
            }
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
        .padding(.horizontal, 12)
        .padding(.vertical, 9)
        .background(Color.primary.opacity(0.05), in: RoundedRectangle(cornerRadius: 8, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 8, style: .continuous).strokeBorder(Theme.hairline))
    }
}

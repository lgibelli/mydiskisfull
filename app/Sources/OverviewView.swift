// The overview: how full the disk is and where it's heading, in a sentence;
// what needs attention, each with what to do; the last days at a glance;
// and the fixes that free the most.

import SwiftUI

struct OverviewPage: View {
    @ObservedObject var monitor: Monitor

    var body: some View {
        Page(title: "Overview", subtitle: subtitle) {
            if let s = monitor.status {
                Hero(status: s, health: monitor.health)
                AttentionSection(monitor: monitor)
                HStack(alignment: .top, spacing: 16) {
                    TrendGlance(monitor: monitor)
                    GrewGlance(monitor: monitor)
                }
                FixesGlance(monitor: monitor)
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

// MARK: - The hero

private struct Hero: View {
    let status: Status
    let health: Health

    var body: some View {
        let s = status
        let used = Double(s.total - s.avail) / Double(max(s.total, 1))
        HStack(spacing: 34) {
            CapacityRing(used: used, colors: health.ring, lineWidth: 18) {
                VStack(spacing: 2) {
                    Text(Format.bytes(s.avail))
                        .font(.system(size: 30, weight: .bold, design: .rounded))
                        .monospacedDigit()
                        .contentTransition(.numericText())
                    Text("free of \(Format.bytes(s.total))")
                        .font(.callout)
                        .foregroundStyle(.secondary)
                }
            }
            .frame(width: 196, height: 196)

            VStack(alignment: .leading, spacing: 12) {
                HealthPill(health: health)
                VStack(alignment: .leading, spacing: 6) {
                    Text(Headline.title(s))
                        .font(.system(size: 30, weight: .bold, design: .rounded))
                        .fixedSize(horizontal: false, vertical: true)
                    Text(Headline.detail(s))
                        .font(.title3)
                        .foregroundStyle(.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                }
                HStack(alignment: .top, spacing: 32) {
                    Fact(label: "Used", value: "\(Format.bytes(s.total - s.avail)) · \(Int((used * 100).rounded()))%")
                    let pace = Headline.pace(s)
                    Fact(label: pace.label, value: pace.value)
                    if let f = s.forecast, !f.steps.isEmpty {
                        Fact(label: "Cleaned up", value: Format.bytes(f.steps.reduce(0) { $0 + $1.freed }))
                    } else {
                        Fact(label: "Samples", value: s.history.samples.formatted())
                    }
                }
                .padding(.top, 6)
            }
            Spacer(minLength: 0)
        }
        .card(padding: 28, tint: health.tint)
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
                    AttentionCard(symbol: "lock.fill", color: .blue,
                                  title: "Let MyDiskIsFull see the whole disk",
                                  text: "Without Full Disk Access, macOS keeps the watcher out of other apps' folders, so what grows there goes unseen. It only ever reads sizes, never what's in a file.") {
                        Button("Open Privacy Settings…") { Privacy.openSettings() }
                            .buttonStyle(.borderedProminent)
                    }
                }
                ForEach(s?.incidents ?? []) { e in
                    AttentionCard(symbol: e.incident.kind == .growth ? "flame.fill" : "arrow.triangle.2.circlepath",
                                  color: .red, title: title(e), text: e.sentence,
                                  badge: "\(Format.bytes(e.incident.perDay)) a day") {
                        FixActions(fix: e.fix, line: e.fixLine, monitor: monitor)
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

    private func title(_ e: Episode) -> String {
        let i = e.incident
        switch i.kind {
        case .growth: return "\(URL(fileURLWithPath: i.subject).lastPathComponent) keeps growing"
        case .churn: return "\(i.subject) keeps rewriting the disk"
        }
    }
}

private struct AllClear: View {
    var body: some View {
        HStack(spacing: 16) {
            Image(systemName: "checkmark.seal.fill")
                .font(.system(size: 30))
                .foregroundStyle(.white, Color.green.gradient)
            VStack(alignment: .leading, spacing: 3) {
                Text("Nothing out of the ordinary").font(.headline)
                Text("No folder is running away and no app is churning the disk. MyDiskIsFull keeps watching in the background and tells you when that changes.")
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

/// What to do about a cause: a tweak to apply, a command to copy (never
/// run), or what to change by hand.
struct FixActions: View {
    let fix: Fix
    let line: String
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
                Text(note).font(.callout).fixedSize(horizontal: false, vertical: true)
                CommandBox(command: command) { monitor.copy(command) }
            }
        case .steps(let text):
            Label {
                Text(text).fixedSize(horizontal: false, vertical: true)
            } icon: {
                Image(systemName: "hand.point.right.fill").foregroundStyle(.blue)
            }
            .font(.callout)
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

// MARK: - At a glance

private struct TrendGlance: View {
    @ObservedObject var monitor: Monitor

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            SectionHeader(title: "Free space", subtitle: "The last 3 days") {
                MoreLink(title: "Trends") { Navigator.shared.pane = .trends }
            }
            if let series = monitor.detail?.series, series.count >= 2 {
                Sparkline(series: series, color: monitor.health == .calm ? .accentColor : monitor.health.tint)
                    .frame(height: 120)
            } else {
                Text("The chart starts once the watcher has a few samples.")
                    .font(.callout).foregroundStyle(.secondary)
                    .frame(maxWidth: .infinity, minHeight: 120)
            }
        }
        .card()
    }
}

private struct GrewGlance: View {
    @ObservedObject var monitor: Monitor

    var body: some View {
        let rows = (monitor.detail?.growth24h ?? []).filter { $0.bytes > 0 }.prefix(4)
        VStack(alignment: .leading, spacing: 12) {
            SectionHeader(title: "Grew the most", subtitle: "The last 24 hours") {
                MoreLink(title: "Trends") { Navigator.shared.pane = .trends }
            }
            if rows.isEmpty {
                Text("Nothing grew much.")
                    .font(.callout).foregroundStyle(.secondary)
                    .frame(maxWidth: .infinity, minHeight: 120)
            } else {
                VStack(spacing: 11) {
                    ForEach(Array(rows.enumerated()), id: \.offset) { _, g in
                        BarRow(name: URL(fileURLWithPath: g.owner).lastPathComponent,
                               detail: Format.path(URL(fileURLWithPath: g.owner).deletingLastPathComponent().path),
                               value: g.bytes, largest: rows.first?.bytes ?? 1, color: .orange,
                               help: Format.path(g.owner))
                    }
                }
                .frame(minHeight: 120, alignment: .top)
            }
        }
        .card()
    }
}

private struct FixesGlance: View {
    @ObservedObject var monitor: Monitor

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            SectionHeader(title: "Ways to free space", subtitle: "Largest first, each with what you give up") {
                MoreLink(title: "All Fixes") { Navigator.shared.pane = .fixes }
            }
            if let list = monitor.suggestions {
                if list.isEmpty {
                    Text("Nothing to suggest: no runaway fillers, and nothing a tweak would stop takes much space.")
                        .font(.callout).foregroundStyle(.secondary)
                } else {
                    VStack(spacing: 0) {
                        ForEach(Array(list.prefix(3).enumerated()), id: \.offset) { i, s in
                            if i > 0 { Divider().padding(.leading, 44) }
                            Button { Navigator.shared.pane = .fixes } label: {
                                HStack(spacing: 12) {
                                    IconTile(symbol: s.symbol, color: s.tint, size: 30)
                                    VStack(alignment: .leading, spacing: 2) {
                                        Text(s.headline).font(.callout.weight(.medium)).lineLimit(1)
                                        Text(s.fixKind).font(.caption).foregroundStyle(.secondary)
                                    }
                                    Spacer(minLength: 8)
                                    if let size = s.space {
                                        Text(Format.bytes(size))
                                            .font(.system(.callout, design: .rounded).weight(.semibold))
                                            .monospacedDigit()
                                    }
                                    Image(systemName: "chevron.right")
                                        .font(.caption.weight(.semibold))
                                        .foregroundStyle(.tertiary)
                                }
                                .padding(.vertical, 9)
                                .contentShape(Rectangle())
                            }
                            .buttonStyle(.plain)
                        }
                    }
                }
            } else {
                HStack(spacing: 8) {
                    ProgressView().controlSize(.small)
                    Text("Measuring the folders that tend to grow…").font(.callout).foregroundStyle(.secondary)
                }
            }
        }
        .card()
    }
}

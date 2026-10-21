// Prevent: the tweaks, each a switch that says what it stops and what you
// give up, and can be undone. And the pieces the Overview shows suggestions
// with: a suggestion's card, its size bars, Keep It.

import SwiftUI

struct PreventPage: View {
    @ObservedObject var monitor: Monitor

    var body: some View {
        Page(title: "Prevent", subtitle: "What keeps the disk from filling up again.") {
            TidySection(monitor: monitor)
            TweakList(monitor: monitor)
        }
        .onAppear { monitor.viewed() }
    }
}

struct SuggestionCard: View {
    let suggestion: Suggestion
    @ObservedObject var monitor: Monitor

    var body: some View {
        let s = suggestion
        HStack(alignment: .top, spacing: 14) {
            IconTile(symbol: s.symbol, color: s.tint, size: 34)
            VStack(alignment: .leading, spacing: 8) {
                HStack(alignment: .firstTextBaseline, spacing: 10) {
                    VStack(alignment: .leading, spacing: 2) {
                        Text(s.headline).font(.headline)
                        if let folder = s.folder {
                            Text(folder).font(.caption).foregroundStyle(.secondary)
                                .lineLimit(1).truncationMode(.middle)
                        }
                    }
                    Spacer(minLength: 8)
                    if let d = s.perDay, d > 0 {
                        Pill(text: "\(Format.bytes(d)) a day", color: .red, symbol: "arrow.up.right")
                    }
                    if let size = s.space, s.frees == nil {
                        Text(Format.bytes(size))
                            .font(.system(.title3, design: .rounded).weight(.semibold))
                            .monospacedDigit()
                    }
                }
                // Its growth shown, not told.
                if let sizes = s.sizes, sizes.count >= 2 {
                    SizeBars(points: sizes, tint: s.tint)
                } else if let detail = s.detail {
                    Text(detail)
                        .font(.callout)
                        .foregroundStyle(.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                }
                if s.frees != nil || s.age != nil {
                    HStack(spacing: 8) {
                        if let f = s.frees, f > 0 {
                            Pill(text: "Frees about \(Format.bytes(f))", color: .green, symbol: "arrow.down.circle.fill")
                        }
                        if let a = s.age {
                            Pill(text: "Files \(Format.age(a)) old on average", color: .secondary, symbol: "clock")
                                .help("How long ago its files were last changed, on average, weighted by size.")
                        }
                    }
                }
                FixActions(fix: s.fix, line: s.headline, action: s.action, monitor: monitor)
                    .padding(.top, 2)
                if let path = s.path {
                    HStack {
                        Spacer()
                        KeepButton(path: path, monitor: monitor)
                    }
                }
            }
        }
        .card(padding: 16)
    }
}

/// A folder's size at the end of each of the last days, and now: how it
/// grew, as bars, each with its size; now in the suggestion's colour.
struct SizeBars: View {
    let points: [Suggestion.SizePoint]
    let tint: Color

    var body: some View {
        let top = max(points.map(\.bytes).max() ?? 1, 1)
        let last = points.last?.ts ?? 0
        HStack(alignment: .bottom, spacing: 14) {
            ForEach(Array(points.enumerated()), id: \.offset) { i, p in
                let now = i == points.count - 1
                VStack(spacing: 4) {
                    Text(Format.bytes(p.bytes))
                        .font(.caption.weight(now ? .semibold : .regular))
                        .monospacedDigit()
                        .foregroundStyle(now ? .primary : .secondary)
                    RoundedRectangle(cornerRadius: 4)
                        .fill(now ? AnyShapeStyle(tint.gradient) : AnyShapeStyle(Color.secondary.opacity(0.25)))
                        .frame(width: 44, height: max(4, 64 * CGFloat(p.bytes) / CGFloat(top)))
                    Text(Self.label(last - p.ts))
                        .font(.caption2)
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                        .fixedSize()
                }
            }
        }
        .padding(.vertical, 2)
        .accessibilityElement(children: .combine)
    }

    /// "Now", "Yesterday", "2 days ago".
    static func label(_ before: Int64) -> String {
        let days = Int((Double(before) / 86_400).rounded())
        switch days {
        case 0: return "Now"
        case 1: return "Yesterday"
        default: return "\(days) days ago"
        }
    }
}

/// Puts what a suggestion is about on the keep list, so nothing suggests
/// deleting it again.
struct KeepButton: View {
    let path: String
    @ObservedObject var monitor: Monitor

    var body: some View {
        Button {
            monitor.keep([path])
        } label: {
            Label("Keep it", systemImage: "pin")
        }
        .buttonStyle(.borderless)
        .controlSize(.small)
        .help("Never suggest deleting, cleaning or moving \((path as NSString).abbreviatingWithTildeInPath) again. Settings lists what you keep.")
    }
}

/// Clean-ups the watcher does by itself once a day, each switched on here:
/// none is on until you turn it on.
private struct TidySection: View {
    @ObservedObject var monitor: Monitor

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            SectionHeader(title: "Keep tidy",
                          subtitle: "Clean-ups MyDiskIsFull does by itself once a day, if you turn them on. Never anything you keep.")
            #if APP_STORE
            FullVersionNote(missing: .cleanups)
                .card(tint: .blue)
            #else
            if let list = monitor.tidies {
                VStack(spacing: 0) {
                    ForEach(Array(list.enumerated()), id: \.element.id) { i, t in
                        if i > 0 { Divider().padding(.leading, 50) }
                        TidyRow(tidy: t, monitor: monitor)
                    }
                }
                .card(padding: 6)
            } else {
                HStack(spacing: 10) {
                    ProgressView().controlSize(.small)
                    Text("Reading what's on…").font(.callout).foregroundStyle(.secondary)
                }
                .card()
            }
            #endif
        }
        #if !APP_STORE
        .task { await monitor.loadTidies() }
        #endif
    }
}

#if !APP_STORE
private struct TidyRow: View {
    let tidy: Tidy
    @ObservedObject var monitor: Monitor

    var body: some View {
        let t = tidy
        let busy = monitor.tidyBusy.contains(t.id)
        HStack(alignment: .top, spacing: 14) {
            IconTile(symbol: symbol, color: t.on ? .green : .gray, size: 30)
            VStack(alignment: .leading, spacing: 4) {
                Text(t.title).font(.body.weight(.medium))
                Text(t.detail).font(.callout).foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
                if let why = t.unavailableWhy, !t.available {
                    Text(why).font(.caption).foregroundStyle(.orange)
                }
                HStack(spacing: 10) {
                    if let last = t.last {
                        Label(lastLine(last), systemImage: last.failed > 0 ? "exclamationmark.circle" : "checkmark.circle")
                            .font(.caption)
                            .foregroundStyle(last.failed > 0 ? Color.orange : Color.secondary)
                    }
                    if t.available {
                        Button("Tidy Now") { monitor.tidyNow(t.id) }
                            .buttonStyle(.link)
                            .font(.caption)
                            .disabled(busy)
                    }
                    if busy { ProgressView().controlSize(.mini) }
                }
                .padding(.top, 2)
            }
            Spacer(minLength: 12)
            Toggle("", isOn: Binding(get: { t.on }, set: { monitor.setTidy(t.id, on: $0) }))
                .labelsHidden()
                .toggleStyle(.switch)
                .disabled(busy || !t.available)
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 12)
    }

    private var symbol: String {
        switch tidy.id {
        case "simulators": "iphone"
        case "incremental": "hammer"
        case "caches": "arrow.triangle.2.circlepath"
        default: "sparkles"
        }
    }

    /// "Freed 3.2 GB on Oct 24", "Nothing to tidy on Oct 24".
    private func lastLine(_ l: Tidy.Last) -> String {
        let day = Date(timeIntervalSince1970: TimeInterval(l.at))
            .formatted(.dateTime.month(.abbreviated).day())
        if l.freed >= 1_000_000 { return "Freed \(Format.bytes(l.freed)) on \(day)" }
        return "\(l.note ?? "Nothing to tidy") on \(day)"
    }
}
#endif

private struct TweakList: View {
    @ObservedObject var monitor: Monitor

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            SectionHeader(title: "Tweaks",
                          subtitle: "Each stops something filling the disk again. MyDiskIsFull keeps what it changes, so you can always turn it back off.")
            #if APP_STORE
            FullVersionNote(missing: .tweaks)
                .card(tint: .blue)
            #else
            if let list = monitor.tweaks?.filter({ !$0.isAbsent }) {
                VStack(spacing: 0) {
                    ForEach(Array(list.enumerated()), id: \.element.id) { i, t in
                        if i > 0 { Divider().padding(.leading, 50) }
                        TweakRow(tweak: t, monitor: monitor)
                    }
                }
                .card(padding: 6)
            } else {
                HStack(spacing: 10) {
                    ProgressView().controlSize(.small)
                    Text("Checking each tweak…").font(.callout).foregroundStyle(.secondary)
                }
                .card()
            }
            #endif
        }
    }
}

#if !APP_STORE
private struct TweakRow: View {
    let tweak: TweakInfo
    @ObservedObject var monitor: Monitor

    var body: some View {
        let t = tweak
        let byUs = t.applied != nil
        HStack(alignment: .top, spacing: 14) {
            IconTile(symbol: t.isApplied ? "checkmark" : "slider.horizontal.3",
                     color: t.isApplied ? .green : .gray, size: 30)
            VStack(alignment: .leading, spacing: 4) {
                HStack(spacing: 8) {
                    Text(t.title).font(.body.weight(.medium))
                    if let note = note { Pill(text: note.0, color: note.1) }
                    if !t.isApplied, let size = t.size, size >= 100_000_000 {
                        Pill(text: "\(Format.bytes(size)) now", color: .blue)
                    }
                }
                Text(t.why).font(.callout).foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
                Text("You give up: \(t.cost)").font(.caption).foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
                if t.status == "waiting for approval" {
                    HStack(spacing: 10) {
                        Text("Nothing changes until you install its profile in System Settings.")
                            .font(.caption).foregroundStyle(.secondary)
                            .fixedSize(horizontal: false, vertical: true)
                        Button("Review and Install…") { monitor.apply(t.id) }
                            .controlSize(.small)
                    }
                    .padding(.top, 2)
                }
            }
            Spacer(minLength: 12)
            Toggle("", isOn: Binding(
                get: { monitor.tweaksChanging[t.id] ?? (t.isApplied || t.status == "waiting for approval") },
                set: { on in on ? monitor.apply(t.id) : monitor.revert(t.id) }
            ))
            .labelsHidden()
            .toggleStyle(.switch)
            // Applied by hand: there is no record of what it replaced. Just
            // changed: until it's measured again.
            .disabled((t.isApplied && !byUs) || monitor.tweaksChanging[t.id] != nil)
            .help(t.isApplied && !byUs
                  ? "Already done: its settings are as this tweak would set them (in System Settings, or by hand). MyDiskIsFull only undoes what it set itself."
                  : "")
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 12)
    }

    private var note: (String, Color)? {
        let t = tweak
        if t.status == "waiting for approval" { return ("Waiting for approval", .orange) }
        if t.status == "partly applied" { return ("Partly applied", .orange) }
        if t.restartPending == true { return ("After a restart", .orange) }
        if t.isApplied && t.applied == nil { return ("Already done, outside MyDiskIsFull", .green) }
        if monitor.undone.contains(where: { $0.id == t.id }) { return ("Undone since", .purple) }
        return nil
    }
}
#endif

extension Suggestion {
    /// A folder that keeps growing: its path, as the cause starts with it.
    private var grower: String? {
        guard cause.hasPrefix("~/") || cause.hasPrefix("/"),
              let r = cause.range(of: " grew ") else { return nil }
        return String(cause[..<r.lowerBound])
    }

    /// A short title: the folder's name when it's about one that grows,
    /// otherwise the cause's first sentence ("Gradle's caches take 8.8 GB").
    /// What to do, as far as its first sentence goes: "Restart the Mac when
    /// it suits you".
    var firstStep: String? {
        guard case .steps(let text) = fix else { return nil }
        return text.split(whereSeparator: { ".:;".contains($0) }).first.map(String.init)
    }

    /// Whether it says to leave it be: nothing to do.
    var leave: Bool {
        if case .leave = fix { return true }
        return false
    }

    var headline: String {
        if let title { return title }
        if let g = grower { return "\((g as NSString).lastPathComponent) keeps growing" }
        let first = cause.components(separatedBy: ". ").first ?? cause
        return first.hasSuffix(".") ? String(first.dropLast()) : first
    }

    /// Where a growing folder is, as the cause writes it ("~/devel").
    var folder: String? {
        grower.map { ($0 as NSString).deletingLastPathComponent }
    }

    /// The rest of the cause (for a growing folder, all of it but the
    /// path); nil when the headline says it all.
    var detail: String? {
        // A title says it; the pill has the numbers.
        if title != nil { return nil }
        if grower != nil, let r = cause.range(of: " grew ") { return "Grew " + cause[r.upperBound...] }
        let parts = cause.components(separatedBy: ". ")
        return parts.count > 1 ? parts.dropFirst().joined(separator: ". ") : nil
    }

    /// What the app can do itself: none in the App Store edition.
    var oneClick: CleanAction? { Edition.appStore ? nil : action }

    var symbol: String {
        if perDay != nil { return "flame.fill" }
        if let a = oneClick { return a.kind == "trash" || a.kind == "delete" ? "trash.fill" : a.symbol }
        switch fix {
        case .tweak: return "wand.and.stars"
        case .run: return "terminal.fill"
        case .steps: return "checklist"
        case .leave: return "leaf.fill"
        }
    }

    var tint: Color {
        if perDay != nil { return .red }
        switch fix {
        case .tweak: return .green
        case .run: return .indigo
        case .steps: return .blue
        case .leave: return .teal
        }
    }

    /// What kind of fix, for a one-line list.
    var fixKind: String {
        if let a = oneClick { return "One click: \(a.label)" }
        switch fix {
        case .tweak: return Edition.appStore ? "A setting to change" : "One-click fix"
        case .run: return "A command to run once"
        case .steps: return "Steps to take"
        case .leave: return "Best left alone"
        }
    }
}

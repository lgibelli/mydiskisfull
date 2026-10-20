// Fixes: what would free space now and keep it from filling again, largest
// first, each with what to do; then the tweaks, each a switch that says what
// it stops and what you give up, and can be undone.

import SwiftUI

struct FixesPage: View {
    @ObservedObject var monitor: Monitor

    var body: some View {
        Page(title: "Fixes", subtitle: "Free space now, and keep it from filling up again.") {
            Recommendations(monitor: monitor)
            TweakList(monitor: monitor)
        }
        .onAppear { monitor.viewed() }
    }
}

private struct Recommendations: View {
    @ObservedObject var monitor: Monitor

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            SectionHeader(title: "Recommended", subtitle: "From what grows on this Mac and what takes the most space.") {
                if monitor.measuring {
                    ProgressView().controlSize(.small)
                }
            }
            if let list = monitor.suggestions {
                if list.isEmpty {
                    HStack(spacing: 14) {
                        IconTile(symbol: "sparkles", color: .green, size: 34)
                        VStack(alignment: .leading, spacing: 2) {
                            Text("Nothing to suggest").font(.headline)
                            Text("No runaway fillers, and nothing a tweak would stop takes much space.")
                                .font(.callout).foregroundStyle(.secondary)
                        }
                    }
                    .card()
                }
                ForEach(Array(list.enumerated()), id: \.offset) { _, s in
                    SuggestionCard(suggestion: s, monitor: monitor)
                }
            } else {
                HStack(spacing: 10) {
                    ProgressView().controlSize(.small)
                    Text("Measuring the folders that tend to grow. This takes a few seconds.")
                        .font(.callout).foregroundStyle(.secondary)
                }
                .card()
            }
        }
    }
}

private struct SuggestionCard: View {
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
                    if let size = s.space {
                        Text(Format.bytes(size))
                            .font(.system(.title3, design: .rounded).weight(.semibold))
                            .monospacedDigit()
                    }
                }
                if let detail = s.detail {
                    Text(detail)
                        .font(.callout)
                        .foregroundStyle(.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                }
                FixActions(fix: s.fix, line: s.headline, monitor: monitor)
                    .padding(.top, 2)
            }
        }
        .card(padding: 16)
    }
}

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
            }
            Spacer(minLength: 12)
            Toggle("", isOn: Binding(
                get: { t.isApplied || t.status == "waiting for approval" },
                set: { on in on ? monitor.apply(t.id) : monitor.revert(t.id) }
            ))
            .labelsHidden()
            .toggleStyle(.switch)
            // Applied by hand: there is no record of what it replaced.
            .disabled(t.isApplied && !byUs)
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 12)
    }

    private var note: (String, Color)? {
        let t = tweak
        if t.status == "waiting for approval" { return ("Waiting for approval", .orange) }
        if t.status == "partly applied" { return ("Partly applied", .orange) }
        if t.restartPending == true { return ("After a restart", .orange) }
        if t.isApplied && t.applied == nil { return ("Set outside MyDiskIsFull", .secondary) }
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
    var headline: String {
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
        if grower != nil, let r = cause.range(of: " grew ") { return "Grew " + cause[r.upperBound...] }
        let parts = cause.components(separatedBy: ". ")
        return parts.count > 1 ? parts.dropFirst().joined(separator: ". ") : nil
    }

    var symbol: String {
        if perDay != nil { return "flame.fill" }
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
        switch fix {
        case .tweak: return Edition.appStore ? "A setting to change" : "One-click fix"
        case .run: return "A command to run once"
        case .steps: return "Steps to take"
        case .leave: return "Best left alone"
        }
    }
}

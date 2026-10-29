// Prevention: what keeps the disk from filling up again, as switches:
// clean-ups MyDiskIsFull does by itself once a day, and tweaks, each a
// setting that says what it stops and what you give up, and can be undone.
// And the pieces suggestions are shown with: their size bars, their titles.

import SwiftUI

struct PreventionPage: View {
    @ObservedObject var monitor: Monitor

    var body: some View {
        Form {
            #if APP_STORE
            Section { FullVersionNote(missing: .tweaks) }
            #else
            Section {
                if let list = monitor.tidies {
                    ForEach(list) { TidyRow(tidy: $0, monitor: monitor) }
                } else {
                    ProgressView().controlSize(.small)
                }
            } header: {
                Text("Automatic Clean-ups")
            } footer: {
                Text("MyDiskIsFull does these once a day, while it watches. Never anything you keep.")
                    .foregroundStyle(.secondary)
            }
            Section {
                if let list = monitor.tweaks?.filter({ !$0.isAbsent }) {
                    ForEach(list) { TweakRow(tweak: $0, monitor: monitor) }
                } else {
                    HStack(spacing: 8) {
                        ProgressView().controlSize(.small)
                        Text("Checking each setting…").foregroundStyle(.secondary)
                    }
                }
            } header: {
                Text("Settings That Stop the Filling")
            } footer: {
                Text("Each changes a setting of macOS or an app so that it stops filling the disk. MyDiskIsFull keeps what it replaced: turn it off to put it back.")
                    .foregroundStyle(.secondary)
            }
            #endif
        }
        .formStyle(.grouped)
        .navigationTitle("Prevention")
        .onAppear { monitor.viewed() }
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
        Toggle(isOn: Binding(get: { t.on }, set: { monitor.setTidy(t.id, on: $0) })) {
            VStack(alignment: .leading, spacing: 3) {
                Text(t.title)
                Text(t.detail)
                    .font(.subheadline)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
                if let why = t.unavailableWhy, !t.available {
                    Text(why).font(.subheadline).foregroundStyle(.orange)
                }
                HStack(spacing: 8) {
                    if let last = t.last {
                        Text(lastLine(last))
                            .foregroundStyle(last.failed > 0 ? Color.orange : Color.secondary)
                    }
                    if t.available {
                        Button("Tidy Now") { monitor.tidyNow(t.id) }
                            .buttonStyle(.link)
                            .disabled(busy)
                    }
                    if busy { ProgressView().controlSize(.mini) }
                }
                .font(.subheadline)
            }
        }
        .toggleStyle(.switch)
        .disabled(busy || !t.available)
        .padding(.vertical, 2)
    }

    /// "Freed 3.2 GB on Oct 24", "Nothing to tidy on Oct 24".
    private func lastLine(_ l: Tidy.Last) -> String {
        let day = Date(timeIntervalSince1970: TimeInterval(l.at))
            .formatted(.dateTime.month(.abbreviated).day())
        if l.freed >= 1_000_000 { return "Freed \(Format.bytes(l.freed)) on \(day)." }
        return "\(l.note ?? "Nothing to tidy") on \(day)."
    }
}

private struct TweakRow: View {
    let tweak: TweakInfo
    @ObservedObject var monitor: Monitor

    var body: some View {
        let t = tweak
        let byUs = t.applied != nil
        Toggle(isOn: Binding(
            get: { monitor.tweaksChanging[t.id] ?? (t.isApplied || t.status == "waiting for approval") },
            set: { on in on ? monitor.apply(t.id) : monitor.revert(t.id) }
        )) {
            VStack(alignment: .leading, spacing: 3) {
                HStack(spacing: 6) {
                    Text(t.title)
                    if let note { Text(note.0).font(.subheadline).foregroundStyle(note.1) }
                }
                Text(t.why)
                    .font(.subheadline)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
                Text("You give up: \(t.cost)")
                    .font(.subheadline)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
                if !t.isApplied, let size = t.size, size >= 100_000_000 {
                    Text("\(Format.bytes(size)) there now.").font(.subheadline).foregroundStyle(.secondary)
                }
                if t.status == "waiting for approval" {
                    HStack(spacing: 8) {
                        Text("Nothing changes until you install its profile in System Settings.")
                            .foregroundStyle(.secondary)
                        Button("Review and Install…") { monitor.apply(t.id) }
                            .buttonStyle(.link)
                    }
                    .font(.subheadline)
                }
            }
        }
        .toggleStyle(.switch)
        // Applied by hand: there is no record of what it replaced. Just
        // changed: until it's measured again.
        .disabled((t.isApplied && !byUs) || monitor.tweaksChanging[t.id] != nil)
        .help(t.isApplied && !byUs
              ? "Already done: its settings are as this tweak would set them (in System Settings, or by hand). MyDiskIsFull only undoes what it set itself."
              : "")
        .padding(.vertical, 2)
    }

    private var note: (String, Color)? {
        let t = tweak
        if t.status == "waiting for approval" { return ("Waiting for approval", .orange) }
        if t.status == "partly applied" { return ("Partly applied", .orange) }
        if t.restartPending == true { return ("After a restart", .orange) }
        if t.isApplied && t.applied == nil { return ("Already done", .secondary) }
        if monitor.undone.contains(where: { $0.id == t.id }) { return ("Undone since", .purple) }
        return nil
    }
}
#endif

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
}

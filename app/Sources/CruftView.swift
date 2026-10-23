// Cruft: what nothing has opened or changed in a long time, biggest first.
// First folders (apps not opened, folders unchanged, from the whole-disk
// scan's items), then big files of yours (`old_files`: your own folders,
// not macOS, ~/Library, apps' insides or build output). How big and how old
// is chosen here. Each can go to the Trash (`mdif clean` checks it again)
// or be kept.

import AppKit
import SwiftUI

struct CruftPage: View {
    @ObservedObject var monitor: Monitor
    /// The smallest file shown, on a log scale: 50 MB to 10 GB.
    @AppStorage("cruft.minBytes") private var minBytes: Double = 100_000_000
    @AppStorage("cruft.minDays") private var minDays: Int = 365

    private static let ages: [(Int, String)] = [(182, "6 months"), (365, "1 year"), (730, "2 years"), (1825, "5 years")]

    /// "1 year", as the picker says it.
    static func age(_ days: Int) -> String {
        ages.first { $0.0 == days }?.1 ?? Format.age(Int64(days) * 86_400)
    }

    var body: some View {
        Page(title: "Cruft", subtitle: "What nothing has opened or changed in a long time.") {
            controls
            if monitor.spaceReadable, let r = monitor.space {
                let folders = SpaceText.forgotten(r, min: Int64(minBytes), days: minDays, limit: 12)
                if !folders.isEmpty {
                    HighlightCard(title: "Forgotten folders",
                                  subtitle: "Apps not opened and folders not changed in that long. Keep what you need; the rest can go.",
                                  symbol: "clock.badge.questionmark", tint: .orange,
                                  items: folders, size: { $0.bytes }, monitor: monitor)
                }
            }
            if !monitor.spaceReadable {
                Text("This needs the whole disk: choose it in Settings.")
                    .font(.callout).foregroundStyle(.secondary).card()
            } else if let r = monitor.space, let all = r.oldFiles {
                let files = filtered(all, measured: r.measured)
                summary(files)
                if files.isEmpty {
                    ContentUnavailableView("No file that old and that big", systemImage: "sparkles",
                                           description: Text("Try a smaller size or a shorter time."))
                        .frame(minHeight: 220)
                } else {
                    VStack(spacing: 0) {
                        ForEach(Array(files.prefix(200).enumerated()), id: \.element.id) { i, f in
                            if i > 0 { Divider().padding(.leading, 52) }
                            CruftRow(file: f, now: r.measured, monitor: monitor)
                        }
                    }
                    .card(padding: 8)
                }
            } else if monitor.space != nil {
                HStack(spacing: 10) {
                    Text("This scan was made before Cruft: measure again to see it.")
                        .font(.callout).foregroundStyle(.secondary)
                    Spacer()
                    Button("Measure Again") { Task { await monitor.loadSpace(refresh: true) } }
                }
                .card()
            } else {
                HStack(spacing: 10) {
                    ProgressView().controlSize(.small)
                    Text("Measuring the whole disk: about a minute the first time.")
                        .font(.callout).foregroundStyle(.secondary)
                }
                .card()
            }
        }
        .task { await monitor.loadSpace() }
    }

    private var controls: some View {
        HStack(alignment: .center, spacing: 24) {
            VStack(alignment: .leading, spacing: 4) {
                Text("At least \(Format.bytes(Int64(minBytes)))")
                    .font(.callout.weight(.semibold)).monospacedDigit()
                Slider(value: Binding(
                    get: { log10(minBytes) },
                    set: { minBytes = Self.rounded(pow(10, $0)) }
                ), in: log10(50_000_000)...log10(10_000_000_000))
                .frame(minWidth: 220)
            }
            VStack(alignment: .leading, spacing: 4) {
                Text("Unused for at least").font(.callout.weight(.semibold))
                Picker("Unused for", selection: $minDays) {
                    ForEach(Self.ages, id: \.0) { Text($0.1).tag($0.0) }
                }
                .pickerStyle(.segmented)
                .labelsHidden()
                .fixedSize()
            }
            Spacer(minLength: 0)
        }
        .card(padding: 16)
    }

    private func summary(_ files: [SpaceReport.OldFile]) -> some View {
        let total = files.reduce(0) { $0 + $1.bytes }
        let frees = files.filter { !$0.kept }.reduce(0) { $0 + $1.frees }
        return HStack(spacing: 12) {
            IconTile(symbol: "archivebox.fill", color: .brown, size: 34)
            VStack(alignment: .leading, spacing: 2) {
                Text("Old files: \(Format.bytes(total)) in \(files.count) file\(files.count == 1 ? "" : "s")")
                    .font(.title3.weight(.semibold))
                Text("Moving the ones you don't keep to the Trash frees about \(Format.bytes(frees)), once it's emptied. \"Last opened\" is as macOS records it, which isn't every read.")
                    .font(.caption).foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
            Spacer(minLength: 0)
        }
    }

    private func filtered(_ all: [SpaceReport.OldFile], measured: Int64) -> [SpaceReport.OldFile] {
        let cutoff = measured - Int64(minDays) * 86_400
        return all.filter { $0.bytes >= Int64(minBytes) && $0.lastUsed <= cutoff }
            .sorted { $0.bytes > $1.bytes }
    }

    /// 50 MB, 100 MB, 200 MB, 500 MB, 1 GB… as the slider moves, roughly.
    static func rounded(_ v: Double) -> Double {
        let p = pow(10, floor(log10(v)))
        let m = v / p
        let step: Double = m < 1.5 ? 1 : m < 3.5 ? 2 : m < 7.5 ? 5 : 10
        return step * p
    }
}

private struct CruftRow: View {
    let file: SpaceReport.OldFile
    let now: Int64
    @ObservedObject var monitor: Monitor

    var body: some View {
        let f = file
        HStack(alignment: .center, spacing: 12) {
            Image(nsImage: NSWorkspace.shared.icon(forFile: f.path))
                .resizable().frame(width: 32, height: 32)
            VStack(alignment: .leading, spacing: 2) {
                HStack(spacing: 6) {
                    Text(f.name).font(.callout.weight(.medium)).lineLimit(1).truncationMode(.middle)
                    if f.kept { Pill(text: "Kept", color: .secondary, symbol: "pin.fill") }
                }
                HStack(spacing: 4) {
                    Text(Format.path((f.path as NSString).deletingLastPathComponent))
                        .font(.caption).foregroundStyle(.secondary)
                        .lineLimit(1).truncationMode(.middle)
                    CopyPathButton(path: f.path)
                }
                Text(used).font(.caption).foregroundStyle(.secondary)
            }
            Spacer(minLength: 8)
            Text(Format.bytes(f.bytes))
                .font(.system(.callout, design: .rounded).weight(.semibold))
                .monospacedDigit()
            Button {
                NSWorkspace.shared.activateFileViewerSelecting([URL(fileURLWithPath: f.path)])
            } label: {
                Image(systemName: "folder")
            }
            .buttonStyle(.borderless)
            .help("Show in Finder")
            if !f.kept {
                KeepButton(path: f.path, monitor: monitor)
                #if !APP_STORE
                if let a = f.action {
                    let busy = monitor.cleaning.contains(f.path)
                    Button {
                        monitor.clean(a)
                    } label: {
                        Label("Move to Trash", systemImage: "trash")
                    }
                    .controlSize(.small)
                    .disabled(busy)
                }
                #else
                if let c = f.action?.shellCommand {
                    Button("Copy Command") { monitor.copy(c) }
                        .buttonStyle(.link)
                        .font(.caption)
                        .help("\(c)\nPaste it into Terminal to move the file to the Trash.")
                }
                #endif
            }
        }
        .padding(.horizontal, 8)
        .padding(.vertical, 8)
    }

    /// "Last opened 3 years ago, changed 2019".
    private var used: String {
        var parts: [String] = []
        if let a = file.accessed, a > 0 { parts.append("Last opened \(Format.age(now - a)) ago") }
        if let m = file.modified, m > 0 {
            let year = Calendar.current.component(.year, from: Date(timeIntervalSince1970: TimeInterval(m)))
            parts.append("changed in \(year)")
        }
        return parts.joined(separator: ", ")
    }
}

// Old Files: what nothing has opened or changed in a long time, in one
// table, biggest first: folders (apps not opened, folders unchanged, from
// the whole-disk scan's items) and big files of yours (`old_files`: your own
// folders, not macOS, ~/Library, apps' insides or build output). How big and
// how old is chosen above it. What's selected can go to the Trash (`mdif
// clean` checks each again) or be kept.

import AppKit
import SwiftUI

/// A row: a forgotten folder or an old file.
private struct OldRow: Identifiable {
    let id: String
    let name: String
    let path: String
    /// Where it is.
    let folder: String
    let bytes: Int64
    /// Last opened or changed, whichever is later.
    let lastUsed: Int64
    /// "Not opened", "Unchanged", "Last opened".
    let how: String
    let action: CleanAction?
    /// A folder, not a file.
    let isFolder: Bool
}

struct OldFilesPage: View {
    @ObservedObject var monitor: Monitor
    @AppStorage("cruft.minBytes") private var minBytes: Double = 100_000_000
    @AppStorage("cruft.minDays") private var minDays: Int = 365
    @State private var selection: Set<String> = []
    @State private var sort = [KeyPathComparator(\OldRow.bytes, order: .reverse)]

    static let ages: [(Int, String)] = [(182, "6 months"), (365, "1 year"), (730, "2 years"), (1825, "5 years")]
    private static let sizes: [Double] = [50_000_000, 100_000_000, 500_000_000, 1_000_000_000, 5_000_000_000]

    /// "1 year", as the picker says it.
    static func age(_ days: Int) -> String {
        ages.first { $0.0 == days }?.1 ?? Format.age(Int64(days) * 86_400)
    }

    var body: some View {
        Group {
            if !monitor.spaceReadable {
                ContentUnavailableView("Needs the whole disk", systemImage: "internaldrive",
                                       description: Text("Choose the whole disk in Settings to see what's old on it."))
            } else if let r = monitor.space {
                let rows = rows(r).sorted(using: sort)
                VStack(spacing: 0) {
                    filters
                    Divider()
                    if rows.isEmpty {
                        ContentUnavailableView("Nothing that old and that big", systemImage: "sparkles",
                                               description: Text("Try a smaller size or a shorter time."))
                    } else {
                        table(rows)
                        Divider()
                        bottomBar(rows)
                    }
                }
            } else {
                ContentUnavailableView {
                    Label("Measuring the whole disk", systemImage: "internaldrive")
                } description: {
                    Text("About a minute the first time.")
                } actions: {
                    ProgressView().controlSize(.small)
                }
            }
        }
        .navigationTitle("Old Files")
        .navigationSubtitle(monitor.space.map { "Measured \(Format.ago($0.measured))" } ?? "")
        .trailingToolbar {
            MeasureButton(measuring: monitor.spaceMeasuring) {
                Task { await monitor.loadSpace(refresh: true) }
            }
        }
        .task { await monitor.loadSpace() }
    }

    private var filters: some View {
        HStack(spacing: 18) {
            Picker("Larger than", selection: $minBytes) {
                ForEach(Self.sizes, id: \.self) { Text(Format.bytes($0)).tag($0) }
            }
            .fixedSize()
            Picker("Not used for", selection: $minDays) {
                ForEach(Self.ages, id: \.0) { Text($0.1).tag($0.0) }
            }
            .fixedSize()
            Spacer()
        }
        .pickerStyle(.menu)
        .padding(.horizontal, 16)
        .padding(.vertical, 9)
    }

    private func table(_ rows: [OldRow]) -> some View {
        Table(rows, selection: $selection, sortOrder: $sort) {
            TableColumn("Name", value: \.name) { r in
                HStack(spacing: 6) {
                    FileIcon(path: r.path, size: 16, folder: r.isFolder)
                    Text(r.name).lineLimit(1).truncationMode(.middle)
                }
                .help(Format.path(r.path))
            }
            .width(min: 160, ideal: 300)
            TableColumn("Where", value: \.folder) { r in
                Text(Format.path(r.folder))
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                    .truncationMode(.head)
            }
            .width(min: 100, ideal: 240)
            TableColumn("Last Used", value: \.lastUsed) { r in
                Text("\(r.how) \(Format.age(now - r.lastUsed)) ago")
                    .foregroundStyle(.secondary)
                    .help(Date(timeIntervalSince1970: TimeInterval(r.lastUsed)).formatted(date: .long, time: .omitted))
            }
            .width(min: 110, ideal: 160)
            TableColumn("Size", value: \.bytes) { r in
                Text(Format.bytes(r.bytes))
                    .monospacedDigit()
                    .frame(maxWidth: .infinity, alignment: .trailing)
            }
            .width(min: 64, ideal: 80)
        }
        .contextMenu(forSelectionType: String.self) { ids in
            let picked = rows.filter { ids.contains($0.id) }
            if picked.count == 1, let r = picked.first {
                PathMenuItems(path: r.path, keep: { monitor.keep([r.path]) })
            } else if !picked.isEmpty {
                Button("Show in Finder") {
                    NSWorkspace.shared.activateFileViewerSelecting(picked.map { URL(fileURLWithPath: $0.path) })
                }
                Button("Keep Them") { monitor.keep(picked.map(\.path)) }
            }
            #if !APP_STORE
            if let a = trash(picked) {
                Divider()
                Button("Move to Trash") { monitor.clean(a) }
            }
            #endif
        } primaryAction: { ids in
            NSWorkspace.shared.activateFileViewerSelecting(
                rows.filter { ids.contains($0.id) }.map { URL(fileURLWithPath: $0.path) })
        }
    }

    private func bottomBar(_ rows: [OldRow]) -> some View {
        let picked = rows.filter { selection.contains($0.id) }
        let size = (picked.isEmpty ? rows : picked).reduce(0) { $0 + $1.bytes }
        return HStack(spacing: 10) {
            Text(picked.isEmpty
                 ? "\(rows.count) item\(rows.count == 1 ? "" : "s"), \(Format.bytes(size))"
                 : "\(picked.count) of \(rows.count) selected, \(Format.bytes(size))")
                .foregroundStyle(.secondary)
                .monospacedDigit()
            Spacer()
            Button("Keep") { monitor.keep(picked.map(\.path)); selection = [] }
                .disabled(picked.isEmpty)
                .help("Never suggest deleting it again. Settings lists what you keep.")
            #if APP_STORE
            Button("Copy Command") {
                if let c = trash(picked)?.shellCommand { monitor.copy(c) }
            }
            .disabled(trash(picked) == nil)
            .help("A Terminal command that moves them to the Trash. The full version does it for you.")
            #else
            let busy = picked.contains { monitor.cleaning.contains($0.path) }
            if busy { ProgressView().controlSize(.small) }
            Button("Move to Trash") {
                if let a = trash(picked) { monitor.clean(a); selection = [] }
            }
            .disabled(trash(picked) == nil || busy)
            #endif
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 10)
    }

    /// Everything selected that can go to the Trash, in one clean-up: `mdif
    /// clean` checks each path again.
    private func trash(_ picked: [OldRow]) -> CleanAction? {
        let paths = picked.compactMap { r in r.action?.kind == "trash" ? r.action?.paths : nil }.flatMap { $0 }
        guard !paths.isEmpty, paths.count >= picked.count else { return nil }
        return CleanAction(kind: "trash", label: "Move to Trash", paths: paths, confirm: nil, done: nil)
    }

    private var now: Int64 { monitor.space?.measured ?? Int64(Date().timeIntervalSince1970) }

    /// Folders and files past both thresholds, not kept.
    private func rows(_ r: SpaceReport) -> [OldRow] {
        let span = Int64(minDays) * 86_400
        let folders = SpaceText.forgotten(r, min: Int64(minBytes), days: minDays, limit: .max).map { i in
            let unused = i.has("unused")
            let last = unused ? (i.lastUsed ?? 0) : r.measured - (i.age ?? 0)
            return OldRow(id: i.id, name: i.name, path: i.path,
                          folder: (i.path as NSString).deletingLastPathComponent, bytes: i.bytes,
                          lastUsed: last, how: unused ? "Opened" : "Changed", action: i.action, isFolder: true)
        }
        let files = (r.oldFiles ?? [])
            .filter { !$0.kept && $0.bytes >= Int64(minBytes) && $0.lastUsed <= r.measured - span }
            .map { f in
                OldRow(id: f.id, name: f.name, path: f.path, folder: (f.path as NSString).deletingLastPathComponent,
                       bytes: f.bytes, lastUsed: f.lastUsed,
                       how: (f.accessed ?? 0) >= (f.modified ?? 0) ? "Opened" : "Changed", action: f.action,
                       isFolder: false)
            }
        return folders + files
    }
}

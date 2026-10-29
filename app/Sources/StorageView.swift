// Storage, as System Settings shows it: the disk's bar, then its categories
// (apps, build output, caches, macOS…) biggest first; a category opens on
// its biggest items, each with what marks it (an app not opened in months,
// build output, a cache), its button when there's something to do, and what
// its app says about it behind the lightbulb. What macOS frees by itself
// has a section of its own. The other view, Folders, is the disk map
// (MapView.swift). From `mdif space`, a whole-disk scan it caches.

import AppKit
import SwiftUI
import UniformTypeIdentifiers

enum StorageMode: String, CaseIterable, Identifiable {
    case categories = "Categories", folders = "Folders"
    var id: String { rawValue }
}

struct StoragePage: View {
    @ObservedObject var monitor: Monitor
    @ObservedObject private var nav = Navigator.shared
    @ObservedObject private var map = MapModel.shared
    @AppStorage("storage.mode") private var mode: StorageMode = .categories

    var body: some View {
        NavigationStack(path: $nav.storagePath) {
            Group {
                switch mode {
                case .categories: CategoriesView(monitor: monitor)
                case .folders: FoldersView(model: map)
                }
            }
            .navigationTitle("Storage")
            .navigationSubtitle(subtitle)
            .navigationDestination(for: String.self) { id in
                KindPage(kindID: id, monitor: monitor)
            }
            .toolbar {
                ToolbarItem(placement: .principal) {
                    Picker("View", selection: $mode) {
                        ForEach(StorageMode.allCases) { Text($0.rawValue).tag($0) }
                    }
                    .pickerStyle(.segmented)
                    .fixedSize()
                    .help("By category, or folder by folder")
                }
            }
            .trailingToolbar {
                switch mode {
                case .categories:
                    MeasureButton(measuring: monitor.spaceMeasuring) {
                        Task { await monitor.loadSpace(refresh: true) }
                    }
                case .folders:
                    MeasureButton(measuring: map.running) { Task { await map.measure() } }
                }
            }
        }
    }

    private var subtitle: String {
        switch mode {
        case .categories:
            guard let r = monitor.space else { return "" }
            return "\(Format.bytes(r.volume.used)) used · measured \(Format.ago(r.measured))"
        case .folders:
            guard let at = map.measuredAt else { return "" }
            return "Measured \(Format.ago(Int64(at.timeIntervalSince1970)))"
        }
    }
}

// MARK: - Categories

private struct CategoriesView: View {
    @ObservedObject var monitor: Monitor

    var body: some View {
        Group {
            if !monitor.spaceReadable {
                ReadableNote(monitor: monitor)
            } else if let r = monitor.space {
                Form {
                    Section {
                        RailBar(stops: RailBar.Stop.of(r), total: max(r.volume.total, 1)) {
                            Navigator.shared.show(kind: $0)
                        }
                        .padding(.vertical, 6)
                        if let s = monitor.status, !monitor.spaceMeasuring {
                            ChangedSince(freed: s.avail - r.volume.free, monitor: monitor)
                        }
                    }
                    Section {
                        ForEach(r.kinds.filter { $0.bytes > 0 }.sorted { $0.bytes > $1.bytes }) { k in
                            NavigationLink(value: k.id) { KindRow(kind: k, back: back(r, k)) }
                        }
                        LabeledContent {
                            Text(Format.bytes(r.volume.free)).monospacedDigit()
                        } label: {
                            Label {
                                Text("Available")
                            } icon: {
                                IconTile(symbol: "circle.dashed", color: Color(white: 0.75), size: 24)
                            }
                        }
                    }
                    ComesBackSection(report: r, monitor: monitor)
                }
                .formStyle(.grouped)
            } else if let e = monitor.spaceError, !monitor.spaceMeasuring {
                ContentUnavailableView {
                    Label("Couldn't measure the disk", systemImage: "exclamationmark.triangle")
                } description: {
                    Text(e)
                } actions: {
                    Button("Try Again") { Task { await monitor.loadSpace(refresh: true) } }
                }
            } else {
                ContentUnavailableView {
                    Label("Measuring the whole disk", systemImage: "internaldrive")
                } description: {
                    Text("About a minute the first time. It reads names, sizes and dates, never what's in a file.")
                } actions: {
                    ProgressView().controlSize(.small)
                }
            }
        }
        .task { await monitor.loadSpace() }
    }

    /// What of a category comes back by itself, as the bar draws it apart.
    private func back(_ r: SpaceReport, _ k: SpaceReport.Kind) -> Int64 {
        min(k.bytes, r.byItself.filter { $0.kind == k.id && $0.bytes >= 500_000_000 }.reduce(0) { $0 + $1.bytes })
    }
}

private struct KindRow: View {
    let kind: SpaceReport.Kind
    /// What of it comes back by itself (purgeable, swap, an update).
    var back: Int64 = 0

    var body: some View {
        HStack(spacing: 10) {
            IconTile(symbol: SpaceText.symbol(kind.id), color: SpaceText.color(kind.id), size: 24)
            VStack(alignment: .leading, spacing: 1) {
                Text(kind.title)
                let notes = [kind.reclaimable >= 500_000_000 ? "\(Format.bytes(kind.reclaimable)) you may not need" : nil,
                             back > 0 ? "\(Format.bytes(back)) comes back by itself" : nil].compactMap { $0 }
                if !notes.isEmpty {
                    Text(notes.joined(separator: " · "))
                        .font(.subheadline)
                        .foregroundStyle(.secondary)
                }
            }
            Spacer(minLength: 8)
            Text(Format.bytes(kind.bytes))
                .foregroundStyle(.secondary)
                .monospacedDigit()
        }
    }
}

/// The disk changed since it was measured (something deleted, a download):
/// what's free now is live, the breakdown isn't until it's measured again.
private struct ChangedSince: View {
    let freed: Int64
    @ObservedObject var monitor: Monitor

    var body: some View {
        if abs(freed) >= 1_000_000_000 {
            HStack(spacing: 8) {
                Image(systemName: freed > 0 ? "arrow.down.circle.fill" : "arrow.up.circle.fill")
                    .foregroundStyle(freed > 0 ? .green : .orange)
                Text(freed > 0
                     ? "\(Format.bytes(freed)) more is available than when this was measured."
                     : "\(Format.bytes(-freed)) less is available than when this was measured.")
                Spacer()
                Button("Measure Again") { Task { await monitor.loadSpace(refresh: true) } }
            }
        }
    }
}

/// What macOS gives back by itself (purgeable space, swap, an update that
/// waits for a restart) and the Trash, which someone empties: counted as
/// used, freed without a clean-up.
private struct ComesBackSection: View {
    let report: SpaceReport
    @ObservedObject var monitor: Monitor

    var body: some View {
        let parts = report.comesBack.filter { $0.bytes >= ($0.what == "trash" ? 100_000_000 : 500_000_000) }
        if !parts.isEmpty {
            Section {
                ForEach(parts, id: \.what) { p in
                    HStack(spacing: 10) {
                        VStack(alignment: .leading, spacing: 1) {
                            Text(SpaceText.label(p.what))
                            Text(p.how.prefix(1).uppercased() + p.how.dropFirst() + ".")
                                .font(.subheadline)
                                .foregroundStyle(.secondary)
                        }
                        Spacer(minLength: 8)
                        Text(Format.bytes(p.bytes)).foregroundStyle(.secondary).monospacedDigit()
                        if p.what == "trash" { TrashButton(bytes: p.bytes, monitor: monitor) }
                    }
                }
            } header: {
                HStack(spacing: 4) {
                    Text("Freed Without Deleting Anything")
                    FreeingInfo(parts: report.byItself)
                }
            }
        }
    }
}

/// Empties the Trash (`mdif clean ~/.Trash`), asked first.
private struct TrashButton: View {
    let bytes: Int64
    @ObservedObject var monitor: Monitor

    var body: some View {
        let path = NSHomeDirectory() + "/.Trash"
        let action = CleanAction(
            kind: "empty-trash", label: "Empty Trash", paths: [path],
            confirm: "The \(Format.bytes(bytes)) in the Trash is erased for good. You can't undo this.",
            done: nil)
        #if APP_STORE
        // The App Store edition may only read: Finder empties it.
        if let c = action.shellCommand {
            Button("Copy Command") { monitor.copy(c) }
                .help("\(c)\nOr choose Finder → Empty Trash.")
        }
        #else
        CleanButton(action: action, monitor: monitor)
        #endif
    }
}

/// In the App Sandbox with one folder chosen: the whole disk is needed.
private struct ReadableNote: View {
    @ObservedObject var monitor: Monitor

    var body: some View {
        ContentUnavailableView {
            Label("Needs the whole disk", systemImage: "internaldrive")
        } description: {
            Text("To see where all of the space goes, let \(Edition.name) see the whole disk, not one folder.")
        } actions: {
            #if APP_STORE
            Button("Choose the Disk…") { monitor.chooseDisk() }
            #endif
        }
    }
}

/// Why deleting doesn't always free space at once, behind an (i): what
/// APFS and macOS keep, and when each comes back.
struct FreeingInfo: View {
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
        ("A macOS update",
         "Once it's downloaded, macOS prepares an update before asking for the restart: a second copy of parts of macOS, on volumes beside your files. The restart that installs it removes the old copy."),
        ("Copies that share space",
         "On APFS a copy can share its blocks with the original. Deleting one of them frees only what isn't shared."),
    ]
}

// MARK: - A category

/// A category's biggest items, the first few or all, and what isn't listed,
/// so the rows add up to it.
private struct KindPage: View {
    let kindID: String
    @ObservedObject var monitor: Monitor
    @State private var all = false
    private static let first = 12

    var body: some View {
        if let r = monitor.space, let k = r.kinds.first(where: { $0.id == kindID }) {
            // The first few, and any an app has something to say about.
            let shown = all ? k.items : k.items.enumerated()
                .filter { $0.offset < Self.first || SpaceText.advised($0.element) }.map(\.element)
            let hidden = k.items.count - shown.count
            let rest = k.bytes - shown.reduce(0) { $0 + $1.bytes }
            Form {
                Section {
                    HStack(spacing: 12) {
                        IconTile(symbol: SpaceText.symbol(k.id), color: SpaceText.color(k.id), size: 36)
                        VStack(alignment: .leading, spacing: 2) {
                            Text(k.title).font(.headline)
                            Text(k.reclaimable >= 500_000_000
                                 ? "\(Format.bytes(k.bytes)), \(Format.bytes(k.reclaimable)) of it you may not need"
                                 : Format.bytes(k.bytes))
                                .foregroundStyle(.secondary)
                                .monospacedDigit()
                        }
                        Spacer()
                        KindCaches(kind: k, monitor: monitor)
                    }
                    .padding(.vertical, 2)
                    // What the apps keep isn't in them: it's under App data.
                    if k.id == "apps", let d = r.kinds.first(where: { $0.id == "appdata" }) {
                        NavigationLink(value: d.id) {
                            LabeledContent("What apps keep (data, attachments, caches)") {
                                Text("\(d.title), \(Format.bytes(d.bytes))")
                            }
                        }
                    }
                }
                Section {
                    if k.items.isEmpty {
                        Text("Nothing big enough to list on its own.").foregroundStyle(.secondary)
                    }
                    ForEach(shown) { item in
                        ItemRow(item: item, kind: k.id, suggestion: monitor.suggestion(about: item), monitor: monitor)
                    }
                    if all || hidden > 0 {
                        Button(all ? "Show Fewer" : "Show \(hidden) More") {
                            withAnimation(.snappy) { all.toggle() }
                        }
                        .buttonStyle(.link)
                    }
                    if rest >= 100_000_000 {
                        LabeledContent {
                            Text(Format.bytes(rest)).monospacedDigit()
                        } label: {
                            Text("Everything else")
                            Text(hidden == 0 ? "In folders too small to list on their own."
                                             : "In the folders not shown, and ones too small to list.")
                        }
                    }
                }
            }
            .formStyle(.grouped)
            .navigationTitle(k.title)
            .navigationSubtitle(Format.bytes(k.bytes))
        } else {
            ContentUnavailableView("Not measured yet", systemImage: "internaldrive")
        }
    }
}

/// A category's caches at once: which ones is chosen in a sheet.
private struct KindCaches: View {
    let kind: SpaceReport.Kind
    @ObservedObject var monitor: Monitor
    @State private var picking = false

    var body: some View {
        #if !APP_STORE
        let caches = SpaceText.emptiable(kind)
        if caches.count >= 2 {
            let busy = caches.contains { monitor.cleaning.contains($0.path) }
            HStack(spacing: 8) {
                if busy { ProgressView().controlSize(.small) }
                Button("Empty Caches…") { picking = true }
                    .disabled(busy)
            }
            .sheet(isPresented: $picking) {
                CachePicker(items: caches, monitor: monitor)
            }
        }
        #endif
    }
}

/// One item: its icon, name and what marks it (or where it is), its size,
/// its button, what its app says about it, and the ⋯ menu.
struct ItemRow: View {
    let item: SpaceReport.Item
    let kind: String
    /// A suggestion about it, shown with it.
    let suggestion: Suggestion?
    @ObservedObject var monitor: Monitor

    var body: some View {
        let i = item
        let keep: (() -> Void)? = !i.has("kept") && SpaceText.mayGo(i) ? { monitor.keep([i.path]) } : nil
        HStack(spacing: 10) {
            FileIcon(path: i.path)
            VStack(alignment: .leading, spacing: 1) {
                Text(i.name).lineLimit(1).truncationMode(.middle)
                if let line = subtitle {
                    Text(line)
                        .font(.subheadline)
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                        .truncationMode(.middle)
                }
            }
            Spacer(minLength: 10)
            VStack(alignment: .trailing, spacing: 1) {
                Text(Format.bytes(i.bytes)).monospacedDigit()
                // Nothing to free (macOS's own, a cloud copy's clones) says nothing.
                if i.frees > 0, i.frees + 500_000_000 < i.bytes {
                    Text("frees \(Format.bytes(i.frees))")
                        .font(.caption).foregroundStyle(.secondary).monospacedDigit()
                        .help("Part of it is shared with other files (clones or hard links): deleting it frees only this much.")
                }
            }
            .fixedSize()
            if let a = suggestion?.action ?? i.action ?? (i.has("kept") ? nil : i.about?.action) {
                #if APP_STORE
                if let c = a.shellCommand {
                    Button("Copy Command") { monitor.copy(c) }
                        .help("\(c)\n\nPaste it into Terminal. The full version does it for you.")
                }
                #else
                CleanButton(action: a, monitor: monitor)
                #endif
            }
            if hasAdvice {
                AdviceButton(item: i, suggestion: suggestion, monitor: monitor)
            }
            if !i.path.isEmpty {
                PathMenu(path: i.path, keep: keep)
            }
        }
        .padding(.vertical, 1)
        .contextMenu {
            if !i.path.isEmpty { PathMenuItems(path: i.path, keep: keep) }
        }
    }

    /// What marks it, or else where it is (when its name isn't its path).
    private var subtitle: String? {
        let marks = SpaceText.marks(item).map(\.0)
        if !marks.isEmpty { return marks.joined(separator: " · ") }
        if let s = suggestion, let d = s.detail { return d }
        let shown = Format.path(item.path)
        return shown == item.name || item.path.isEmpty ? item.note : shown
    }

    private var hasAdvice: Bool {
        (item.about != nil && !item.has("kept")) || item.note != nil || suggestion != nil
    }
}

/// An item's icon: an app's own, a folder's or a file's by its type. Never
/// read from the item itself, which for a cloud folder could fetch it.
struct FileIcon: View {
    let path: String
    var size: CGFloat = 24
    /// What it is when its name says nothing (no extension, or one no app
    /// claims, as in "group.com.apple.notes"): a folder, or a file.
    var folder = true

    var body: some View {
        Image(nsImage: Self.image(path, folder: folder))
            .resizable()
            .frame(width: size, height: size)
    }

    @MainActor private static var cache: [String: NSImage] = [:]

    /// From the name alone, never the item: looking would download a cloud
    /// placeholder, or open a folder that asks for permission.
    @MainActor static func image(_ path: String, folder: Bool = true) -> NSImage {
        let ext = (path as NSString).pathExtension.lowercased()
        let type = ext.isEmpty ? nil : UTType(filenameExtension: ext).flatMap { $0.isDynamic ? nil : $0 }
        // An app is on this Mac, not in the cloud: its own icon.
        let key = ext == "app" ? path : (type?.identifier ?? (folder ? "/folder" : "/file"))
        if let done = cache[key] { return done }
        let image: NSImage
        if ext == "app" {
            image = NSWorkspace.shared.icon(forFile: path)
        } else {
            image = NSWorkspace.shared.icon(for: type ?? (folder ? .folder : .data))
        }
        cache[key] = image
        return image
    }
}

/// The lightbulb at the end of an item's row: what its app says about it
/// and how to make it smaller, its note, how it grew, and its fix.
private struct AdviceButton: View {
    let item: SpaceReport.Item
    let suggestion: Suggestion?
    @ObservedObject var monitor: Monitor
    @State private var shown = false

    var body: some View {
        let about = item.has("kept") ? nil : item.about
        Button {
            shown.toggle()
        } label: {
            Image(systemName: about != nil ? "lightbulb" : "info.circle")
        }
        .buttonStyle(.borderless)
        .foregroundStyle(about != nil ? Color.yellow : Color.secondary)
        .help(about.map { "\($0.title): how to make it smaller" } ?? "More about it")
        .popover(isPresented: $shown, arrowEdge: .bottom) {
            VStack(alignment: .leading, spacing: 12) {
                Text(item.name).font(.headline)
                if let note = item.note {
                    Text(Format.inline(note)).foregroundStyle(.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                }
                if let about {
                    AboutBox(about: about, monitor: monitor)
                }
                if let s = suggestion {
                    Text(Format.inline(s.cause)).fixedSize(horizontal: false, vertical: true)
                    if let sizes = s.sizes, sizes.count >= 2 {
                        SizeBars(points: sizes, tint: s.tint)
                        // A folder whose parts are listed on their own rows
                        // shows less than the bars, which measure all of it.
                        if let last = sizes.last?.bytes, Double(last) > Double(item.bytes) * 1.1 {
                            Text("The whole folder, with the parts listed on their own.")
                                .font(.caption).foregroundStyle(.secondary)
                        }
                    }
                    if s.action == nil {
                        FixActions(fix: s.fix, line: s.headline, monitor: monitor)
                    }
                }
            }
            .padding(16)
            .frame(width: 420)
        }
    }
}

/// What an app keeps in an item, from its handler in `mdif`: what it is,
/// how to make it smaller, and a button to the place (the app, or System
/// Settings).
private struct AboutBox: View {
    let about: SpaceReport.About
    @ObservedObject var monitor: Monitor

    var body: some View {
        let target = Self.target(about.open)
        HStack(alignment: .top, spacing: 10) {
            if let app = about.open?.app,
               let url = NSWorkspace.shared.urlForApplication(withBundleIdentifier: app) {
                Image(nsImage: NSWorkspace.shared.icon(forFile: url.path))
                    .resizable().frame(width: 32, height: 32)
            } else {
                Image(systemName: "lightbulb.fill")
                    .font(.title3).foregroundStyle(.yellow)
                    .frame(width: 32, height: 32)
            }
            VStack(alignment: .leading, spacing: 5) {
                HStack(alignment: .firstTextBaseline) {
                    Text(about.title).font(.callout.weight(.semibold))
                    Text(Format.bytes(about.bytes))
                        .font(.callout).monospacedDigit()
                        .foregroundStyle(.secondary)
                }
                Text(Format.inline(about.what))
                    .font(.callout).foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
                Text(Format.inline(about.how))
                    .font(.callout)
                    .fixedSize(horizontal: false, vertical: true)
                #if APP_STORE
                if let action = about.action {
                    LiteCommand(action: action, monitor: monitor)
                }
                #endif
                if let open = about.open, let target {
                    Button(open.label) { NSWorkspace.shared.open(target) }
                        .padding(.top, 2)
                }
            }
        }
    }

    /// Where the button goes: the app, if it's on this Mac, or the URL.
    static func target(_ open: SpaceReport.About.Open?) -> URL? {
        guard let open else { return nil }
        if let app = open.app { return NSWorkspace.shared.urlForApplication(withBundleIdentifier: app) }
        return open.url.flatMap(URL.init(string:))
    }
}

#if !APP_STORE
/// Which caches to empty, each with a checkbox, all checked to start: the
/// sheet is the confirmation, so nothing is asked again.
private struct CachePicker: View {
    let items: [SpaceReport.Item]
    @ObservedObject var monitor: Monitor
    @Environment(\.dismiss) private var dismiss
    @State private var chosen: Set<String> = []

    var body: some View {
        let picked = items.filter { chosen.contains($0.path) }
        let size = picked.reduce(0) { $0 + $1.bytes }
        VStack(alignment: .leading, spacing: 14) {
            Text("Empty which caches?").font(.headline)
            Text("What's in them is deleted, not moved to the Trash. Apps make what they need again, so some are slower for a while and download some of it again. One whose app is open is left until you quit the app.")
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
            List {
                ForEach(items) { item in
                    Toggle(isOn: Binding(
                        get: { chosen.contains(item.path) },
                        set: { if $0 { chosen.insert(item.path) } else { chosen.remove(item.path) } }
                    )) {
                        HStack {
                            VStack(alignment: .leading, spacing: 1) {
                                Text(item.name)
                                Text(Format.path(item.path)).font(.caption).foregroundStyle(.secondary)
                                    .lineLimit(1).truncationMode(.middle)
                            }
                            Spacer(minLength: 12)
                            Text(Format.bytes(item.bytes)).monospacedDigit()
                        }
                    }
                    .toggleStyle(.checkbox)
                }
            }
            .listStyle(.bordered(alternatesRowBackgrounds: true))
            .frame(height: min(320, CGFloat(items.count) * 44 + 8))
            HStack {
                Button(chosen.count == items.count ? "Select None" : "Select All") {
                    chosen = chosen.count == items.count ? [] : Set(items.map(\.path))
                }
                Spacer()
                Button("Cancel", role: .cancel) { dismiss() }
                    .keyboardShortcut(.cancelAction)
                Button(picked.isEmpty ? "Empty" : "Empty \(picked.count) (\(Format.bytes(size)))") {
                    monitor.clean(CleanAction(kind: "empty", label: "Empty Caches",
                                              paths: picked.flatMap { $0.action?.paths ?? [] },
                                              confirm: nil, done: nil))
                    dismiss()
                }
                .keyboardShortcut(.defaultAction)
                .disabled(picked.isEmpty)
            }
        }
        .padding(20)
        .frame(width: 520)
        .onAppear { chosen = Set(items.map(\.path)) }
    }
}
#endif

enum SpaceText {
    static func color(_ kind: String) -> Color {
        switch kind {
        case "macos": .gray
        case "apps": .blue
        case "build": .orange
        case "dev": .indigo
        case "cloud": .cyan
        case "vms": .purple
        case "media": .pink
        case "mail": .teal
        case "caches": .yellow
        case "appdata": .mint
        case "files": .green
        default: Color(white: 0.6)
        }
    }

    static func symbol(_ kind: String) -> String {
        switch kind {
        case "macos": "apple.logo"
        case "apps": "square.grid.2x2.fill"
        case "build": "hammer.fill"
        case "dev": "wrench.and.screwdriver.fill"
        case "cloud": "icloud.fill"
        case "vms": "pc"
        case "media": "photo.fill"
        case "mail": "envelope.fill"
        case "caches": "arrow.triangle.2.circlepath"
        case "appdata": "tray.full.fill"
        case "files": "doc.fill"
        default: "ellipsis"
        }
    }

    /// What's freed without a clean-up, by macOS's names: "Purgeable".
    static func label(_ w: String) -> String {
        switch w {
        case "purgeable": "Purgeable"
        case "swap": "Swap"
        case "update": "macOS update"
        case "trash": "In the Trash"
        default: w.capitalized
        }
    }

    /// The biggest of what looks forgotten, from every kind: unchanged or
    /// not opened for `days` at least, `min` bytes or more.
    static func forgotten(_ r: SpaceReport, min: Int64 = 500_000_000, days: Int = 365,
                          limit: Int = 6) -> [SpaceReport.Item] {
        let span = Int64(days) * 86_400
        let all = r.kinds.flatMap(\.items)
            .filter { i in
                // The Trash has its own button.
                guard !i.has("kept"), i.bytes >= min, !i.path.hasSuffix("/.Trash") else { return false }
                if i.has("cold"), (i.age ?? 0) >= span { return true }
                return i.has("unused") && r.measured - (i.lastUsed ?? 0) >= span
            }
            .sorted { $0.bytes > $1.bytes }
        return Array(all.prefix(limit))
    }

    /// Whether an app says how to make it smaller, and it's worth a look:
    /// listed with its kind even past the first few.
    static func advised(_ i: SpaceReport.Item) -> Bool {
        (i.about?.bytes ?? 0) >= 500_000_000 && !i.has("kept")
    }

    /// A kind's caches that can be emptied, biggest first.
    static func emptiable(_ k: SpaceReport.Kind) -> [SpaceReport.Item] {
        k.items.filter { $0.action?.kind == "empty" }.sorted { $0.bytes > $1.bytes }
    }

    /// Whether it's marked as something that may go: worth a Keep It.
    static func mayGo(_ i: SpaceReport.Item) -> Bool {
        ["unused", "cold", "build", "cache"].contains { i.has($0) }
    }

    /// Its marks, as words: text, colour, symbol.
    static func marks(_ i: SpaceReport.Item) -> [(String, Color, String?)] {
        var out: [(String, Color, String?)] = []
        if i.has("kept") { out.append(("Kept", .secondary, "pin.fill")) }
        if i.has("unused") {
            if let t = i.lastUsed {
                out.append(("Not opened in \(Format.age(Int64(Date().timeIntervalSince1970) - t))", .orange, "moon.zzz.fill"))
            } else {
                out.append(("Never opened", .orange, "moon.zzz.fill"))
            }
        }
        if i.has("cold"), let a = i.age {
            out.append(("Unchanged for \(Format.age(a))", .blue, "snowflake"))
        }
        if i.has("build") { out.append(("Build output", .orange, "hammer.fill")) }
        if i.has("cache") { out.append(("Cache", .yellow, "arrow.triangle.2.circlepath")) }
        return out
    }
}

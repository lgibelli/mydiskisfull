// Storage's Folders view, the disk map: where the space is now, folder by
// folder, measured with APFS private size. A whole-volume scan reads
// metadata only, but every directory on the disk: about 200 s of CPU (most
// of it in the kernel) over a minute or two at background priority, on the
// development Mac. That is 200 times the watcher's budget for an hour, so it
// never runs in the background. It runs while someone is looking: when the
// page opens and the kept one is over 30 minutes old, every hour while it
// stays open, and when asked. The kept map shows meanwhile.

import SwiftUI

/// The map, kept while the app runs: leaving the page doesn't stop a scan or
/// start a second one when it's opened again.
@MainActor
final class MapModel: ObservableObject {
    static let shared = MapModel()

    @Published private(set) var map: DiskMap?
    @Published private(set) var measuredAt: Date?
    @Published private(set) var context = MapContext(growth24h: [])
    @Published private(set) var running = false
    @Published private(set) var started = Date()
    @Published private(set) var error: String?

    /// A map older than this is measured again when the window opens.
    static let staleOnOpen: TimeInterval = 30 * 60
    /// And this often while it stays open.
    static let refreshEvery: TimeInterval = 60 * 60

    private var loaded = false

    func age(at now: Date = Date()) -> TimeInterval? {
        measuredAt.map { now.timeIntervalSince($0) }
    }

    /// Measures in the background if there's no map or it's older than `limit`.
    func refreshIfOld(_ limit: TimeInterval) {
        guard !running, Self.mayMeasure else { return }
        if let age = age(), age < limit { return }
        Task { await measure(automatic: true) }
    }

    /// ~/Library/Caches/<bundle id>/disk-map.json: regenerable, and never
    /// leaves the Mac. Its modification time is when the map was taken.
    private static var keptURL: URL? {
        FileManager.default.urls(for: .cachesDirectory, in: .userDomainMask).first?
            .appendingPathComponent(Bundle.main.bundleIdentifier ?? "io.github.lgibelli.mydiskisfull")
            .appendingPathComponent("disk-map.json")
    }

    func loadKept() async {
        await loadGrowth()
        guard !loaded else { return }
        loaded = true
        guard map == nil, let url = Self.keptURL, let data = try? Data(contentsOf: url) else { return }
        do {
            map = try Wire.decode(DiskMap.self, from: data)
            measuredAt = try url.resourceValues(forKeys: [.contentModificationDateKey]).contentModificationDate
        } catch {
            // Written by another version of the app: measure afresh.
            try? FileManager.default.removeItem(at: url)
        }
    }

    /// What grew comes from the watcher's history, which takes milliseconds.
    private func loadGrowth() async {
        if let d = try? await Core.json(ForecastDetail.self, ["forecast"], timeout: 60) {
            context = MapContext(growth24h: d.growth24h)
        }
    }

    /// In the App Sandbox there's nothing to measure until the disk is
    /// chosen.
    static var mayMeasure: Bool {
        #if APP_STORE
        AppDelegate.monitor?.access.granted == true
        #else
        true
        #endif
    }

    /// `automatic`: nobody pressed a button, so it runs at background
    /// priority, out of the way of whatever the Mac is doing.
    func measure(automatic: Bool = false) async {
        guard !running, Self.mayMeasure else { return }
        running = true
        started = Date()
        defer { running = false }
        await loadGrowth()
        do {
            var args = ["map", "--depth", "6", "--min-mb", "200", "--json"]
            #if APP_STORE
            // A folder rather than the disk: map that.
            if let root = AppDelegate.monitor?.access.mapRoot { args += ["--root", root] }
            #endif
            let data = try await Core.run(args, timeout: 900,
                                          qos: automatic ? .background : .userInitiated)
            let fresh = try Wire.decode(DiskMap.self, from: data)
            map = fresh
            measuredAt = Date()
            error = nil
            if let url = Self.keptURL {
                try? FileManager.default.createDirectory(at: url.deletingLastPathComponent(),
                                                         withIntermediateDirectories: true)
                try? data.write(to: url, options: .atomic)
            }
        } catch is DecodingError {
            self.error = "The bundled mdif printed something this version of the app can't read."
        } catch {
            self.error = error.localizedDescription
        }
    }
}

/// The disk folder by folder (Storage → Folders): the map, and before the
/// first one what it is and the button.
struct FoldersView: View {
    @ObservedObject var model: MapModel

    var body: some View {
        Group {
            if let m = model.map {
                VStack(alignment: .leading, spacing: 8) {
                    MapExplorer(map: m, context: model.context)
                    Text(caption(m))
                        .font(.caption)
                        .foregroundStyle(.secondary)
                    if let error = model.error {
                        Label(error, systemImage: "exclamationmark.triangle").foregroundStyle(.orange).font(.callout)
                    }
                }
                .padding(20)
            } else {
                EmptyMap(model: model)
            }
        }
        .task {
            await model.loadKept()
            model.refreshIfOld(MapModel.staleOnOpen)
            // While the view shows (the task ends when it goes).
            while !Task.isCancelled {
                try? await Task.sleep(for: .seconds(60))
                model.refreshIfOld(MapModel.refreshEvery)
            }
        }
    }

    /// What the sizes are, and what the map doesn't show.
    private func caption(_ m: DiskMap) -> String {
        let t = m.scan.root.totals
        var parts = ["Sizes are what deleting would free"]
        if m.scan.sharedOnce >= 1_000_000_000 {
            parts.append("\(Format.bytes(m.scan.sharedOnce)) more is shared by clones and snapshots")
        }
        if t.datalessLogical >= 1_000_000_000 {
            parts.append("\(Format.bytes(t.datalessLogical)) is in the cloud only, not on this Mac")
        }
        return parts.joined(separator: ". ") + ". Nothing is opened, and cloud files are never downloaded."
    }
}

/// Before the first map: what it is, what it costs, and the button.
private struct EmptyMap: View {
    @ObservedObject var model: MapModel

    var body: some View {
        ContentUnavailableView {
            Label("Map your disk folder by folder", systemImage: "chart.pie")
        } description: {
            Text("It reads every folder's size (never what's in a file) and takes a minute or two. It runs again by itself while this view is open.")
        } actions: {
            if model.running {
                TimelineView(.periodic(from: .now, by: 1)) { ctx in
                    VStack(spacing: 6) {
                        ProgressView().progressViewStyle(.linear).frame(width: 240)
                        Text("Measuring… \(Int(ctx.date.timeIntervalSince(model.started))) s")
                            .font(.caption).foregroundStyle(.secondary).monospacedDigit()
                    }
                }
            } else if !MapModel.mayMeasure {
                #if APP_STORE
                Button("Choose Disk…") { AppDelegate.monitor?.chooseDisk() }
                #endif
            } else {
                Button("Measure Now") { Task { await model.measure() } }
                    .buttonStyle(.borderedProminent)
            }
            if let error = model.error {
                Label(error, systemImage: "exclamationmark.triangle").foregroundStyle(.orange).font(.callout)
            }
        }
    }
}

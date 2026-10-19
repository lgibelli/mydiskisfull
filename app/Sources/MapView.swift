// The disk map: where the space is now, measured with APFS private size and
// reconciled with what APFS says is in use. A whole-volume scan reads
// metadata only, but every directory on the disk: about 200 s of CPU (most
// of it in the kernel) over a minute or two at background priority, on the
// development Mac. That is 200 times the watcher's budget for an hour, so it
// never runs in the background. It runs while someone is looking: when the
// map opens and the kept one is over 30 minutes old, every hour while it
// stays open, and when asked. The kept map shows meanwhile.

import SwiftUI

/// The map, kept across windows: closing the window doesn't stop a scan or
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
        guard !running else { return }
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

    /// `automatic`: nobody pressed a button, so it runs at background
    /// priority, out of the way of whatever the Mac is doing.
    func measure(automatic: Bool = false) async {
        guard !running else { return }
        running = true
        started = Date()
        defer { running = false }
        await loadGrowth()
        do {
            let data = try await Core.run(["map", "--depth", "6", "--min-mb", "200", "--json"], timeout: 900,
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

struct MapView: View {
    @ObservedObject private var model = MapModel.shared

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            HStack(alignment: .top) {
                VStack(alignment: .leading, spacing: 4) {
                    Text("Where the space is").font(.title2.bold())
                    Text("Sizes are what deleting a folder would free: files shared with clones or held by a snapshot are counted once, apart. Nothing is opened, and cloud files are never downloaded.")
                        .font(.callout).foregroundStyle(.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                }
                Spacer()
                Button(model.map == nil ? "Measure" : "Measure Again") { Task { await model.measure() } }
                    .disabled(model.running)
            }
            TimelineView(.periodic(from: .now, by: model.running ? 1 : 60)) { ctx in
                status(now: ctx.date)
            }
            if let error = model.error {
                Label(error, systemImage: "exclamationmark.triangle").foregroundStyle(.orange)
            }
            if let m = model.map {
                Summary(map: m)
                MapExplorer(map: m, context: model.context)
            } else {
                Spacer()
            }
        }
        .padding(20)
        .frame(minWidth: 720, minHeight: 640)
        .task {
            await model.loadKept()
            model.refreshIfOld(MapModel.staleOnOpen)
            // While the window is open (the task ends when it closes).
            while !Task.isCancelled {
                try? await Task.sleep(for: .seconds(60))
                model.refreshIfOld(MapModel.refreshEvery)
            }
        }
    }

    /// When the map was taken, and whether a newer one is on its way.
    @ViewBuilder
    private func status(now: Date) -> some View {
        let taken = model.measuredAt.map { "Measured \(Format.when(Int64($0.timeIntervalSince1970))), \(Format.duration(Int64(now.timeIntervalSince($0)))) ago." }
        if model.running {
            let secs = Int(now.timeIntervalSince(model.started))
            if let taken {
                Label("\(taken) Measuring again, \(secs) s so far (a minute or two); this one shows meanwhile.",
                      systemImage: "arrow.triangle.2.circlepath")
                    .font(.callout).foregroundStyle(.secondary)
            } else {
                ProgressView("Measuring the whole disk… \(secs) s (a minute or two)")
                    .progressViewStyle(.linear)
            }
        } else if let taken {
            Text("\(taken) It's measured again when you open the map after 30 minutes, and every hour while it's open.")
                .font(.callout).foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
        }
    }
}

private struct Summary: View {
    let map: DiskMap

    var body: some View {
        let c = map.container
        let t = map.scan.root.totals
        Grid(alignment: .leading, horizontalSpacing: 14, verticalSpacing: 3) {
            row("Disk", "\(Format.bytes(c.capacity)) · \(Format.bytes(c.capacity - c.free)) used · \(Format.bytes(c.free)) free")
            row("Measured", "\(Format.bytes(map.scan.real)) = \(Format.bytes(t.privateBytes)) private + \(Format.bytes(map.scan.sharedOnce)) shared with clones or snapshots")
            if map.root == "/System/Volumes/Data" {
                row("Not measured", "\(Format.bytes(map.unmeasured)): files only the system can read, file-system metadata, blocks only snapshots hold")
            }
            if t.datalessLogical > 0 {
                row("Cloud-only", "\(Format.bytes(t.datalessLogical)) in \(t.datalessFiles) placeholders, not on this disk")
            }
            if t.purgeable > 0 {
                row("Purgeable", "\(Format.bytes(t.purgeable)) macOS can delete by itself when space runs low")
            }
            row("Snapshots", map.snapshots.isEmpty
                ? "No local Time Machine snapshots"
                : "\(map.snapshots.count) local Time Machine snapshot\(map.snapshots.count == 1 ? "" : "s"), keeping deleted files' space")
        }
        .font(.callout)
    }

    private func row(_ k: String, _ v: String) -> some View {
        GridRow {
            Text(k).foregroundStyle(.secondary)
            Text(v).fixedSize(horizontal: false, vertical: true)
        }
    }
}

// The disk map: where the space is now, measured with APFS private size and
// reconciled with what APFS says is in use. A whole-volume scan reads
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

struct MapPage: View {
    @ObservedObject private var model = MapModel.shared

    var body: some View {
        Page(title: "Disk Map",
             subtitle: "Where the space is now. Sizes are what deleting would free. Nothing is opened, and cloud files are never downloaded.") {
            if let m = model.map {
                StorageCard(map: m, model: model)
                MapExplorer(map: m, context: model.context)
                    .frame(height: 620)
                    .card(padding: 18)
            } else {
                EmptyMap(model: model)
            }
        }
        .task {
            await model.loadKept()
            model.refreshIfOld(MapModel.staleOnOpen)
            // While the page shows (the task ends when it goes).
            while !Task.isCancelled {
                try? await Task.sleep(for: .seconds(60))
                model.refreshIfOld(MapModel.refreshEvery)
            }
        }
    }
}

/// Before the first map: what it is, what it costs, and the button.
private struct EmptyMap: View {
    @ObservedObject var model: MapModel

    var body: some View {
        VStack(spacing: 16) {
            IconTile(symbol: "chart.pie.fill", color: .orange, size: 64)
            Text("Map your disk").font(.title2.weight(.semibold))
            Text("A full measure reads every folder's size (never what's in a file) and takes a minute or two. It runs again by itself while this page is open.")
                .font(.callout).foregroundStyle(.secondary)
                .multilineTextAlignment(.center)
                .frame(maxWidth: 440)
            if model.running {
                TimelineView(.periodic(from: .now, by: 1)) { ctx in
                    VStack(spacing: 6) {
                        ProgressView().progressViewStyle(.linear).frame(width: 260)
                        Text("Measuring… \(Int(ctx.date.timeIntervalSince(model.started))) s")
                            .font(.caption).foregroundStyle(.secondary).monospacedDigit()
                    }
                }
            } else {
                Button("Measure Now") { Task { await model.measure() } }
                    .buttonStyle(.borderedProminent)
                    .controlSize(.large)
            }
            if let error = model.error {
                Label(error, systemImage: "exclamationmark.triangle").foregroundStyle(.orange).font(.callout)
            }
        }
        .padding(.vertical, 56)
        .frame(maxWidth: .infinity)
        .card()
    }
}

/// The disk as one bar, as System Settings draws it: what the scan found,
/// what clones and snapshots share, what only the system sees, what's free.
private struct StorageCard: View {
    let map: DiskMap
    @ObservedObject var model: MapModel

    private struct Part: Identifiable {
        let name: String
        let bytes: Int64
        let color: Color
        var id: String { name }
    }

    var body: some View {
        let c = map.container
        let t = map.scan.root.totals
        let used = c.capacity - c.free
        let other = max(0, used - t.privateBytes - map.scan.sharedOnce)
        let parts = [
            Part(name: "Your files", bytes: t.privateBytes, color: Theme.brand[0]),
            Part(name: "Shared by clones and snapshots", bytes: map.scan.sharedOnce, color: .purple),
            Part(name: "System and unmeasured", bytes: other, color: Color(white: 0.6)),
        ]
        VStack(alignment: .leading, spacing: 14) {
            HStack(alignment: .firstTextBaseline) {
                VStack(alignment: .leading, spacing: 2) {
                    Text(Disk.name).font(.title3.weight(.semibold))
                    Text("\(Format.bytes(used)) used of \(Format.bytes(c.capacity)) · \(Format.bytes(c.free)) free")
                        .font(.callout).foregroundStyle(.secondary).monospacedDigit()
                }
                Spacer()
                status
            }
            GeometryReader { geo in
                HStack(spacing: 2) {
                    ForEach(parts) { p in
                        let w = geo.size.width * Double(p.bytes) / Double(max(c.capacity, 1))
                        if w >= 2 {
                            Rectangle().fill(p.color.gradient).frame(width: w)
                                .help("\(p.name): \(Format.bytes(p.bytes))")
                        }
                    }
                    Rectangle().fill(Color.primary.opacity(0.07))
                }
                .clipShape(RoundedRectangle(cornerRadius: 6, style: .continuous))
            }
            .frame(height: 22)
            HStack(spacing: 20) {
                ForEach(parts) { p in legend(p.color, p.name, p.bytes) }
                legend(Color.primary.opacity(0.15), "Free", c.free)
            }
            .font(.caption)
            let extras = notes(t)
            if !extras.isEmpty {
                HStack(spacing: 8) {
                    ForEach(extras, id: \.0) { Pill(text: $0.0, color: .secondary, symbol: $0.1) }
                }
            }
            if let error = model.error {
                Label(error, systemImage: "exclamationmark.triangle").foregroundStyle(.orange).font(.callout)
            }
        }
        .card(padding: 20)
    }

    private func legend(_ color: Color, _ name: String, _ bytes: Int64) -> some View {
        HStack(spacing: 6) {
            RoundedRectangle(cornerRadius: 3).fill(color).frame(width: 10, height: 10)
            Text(name).foregroundStyle(.secondary)
            Text(Format.bytes(bytes)).monospacedDigit()
        }
    }

    private func notes(_ t: Totals) -> [(String, String)] {
        var out: [(String, String)] = []
        if t.datalessLogical > 0 {
            out.append(("\(Format.bytes(t.datalessLogical)) in the cloud only", "icloud"))
        }
        if t.purgeable > 0 {
            out.append(("\(Format.bytes(t.purgeable)) purgeable", "arrow.3.trianglepath"))
        }
        out.append((map.snapshots.isEmpty
                     ? "No local snapshots"
                     : "\(map.snapshots.count) local snapshot\(map.snapshots.count == 1 ? "" : "s")",
                     "clock.arrow.circlepath"))
        return out
    }

    /// When the map was taken, whether a newer one is on its way, and the button.
    private var status: some View {
        TimelineView(.periodic(from: .now, by: model.running ? 1 : 60)) { ctx in
            HStack(spacing: 10) {
                if model.running {
                    ProgressView().controlSize(.small)
                    Text("Measuring again… \(Int(ctx.date.timeIntervalSince(model.started))) s")
                        .monospacedDigit()
                } else if let at = model.measuredAt {
                    Text("Measured \(Format.ago(Int64(at.timeIntervalSince1970), now: ctx.date))")
                        .help("It's measured again when this page opens after 30 minutes, and every hour while it stays open.")
                }
                Button {
                    Task { await model.measure() }
                } label: {
                    Label("Measure Again", systemImage: "arrow.clockwise")
                }
                .disabled(model.running)
            }
            .font(.callout)
            .foregroundStyle(.secondary)
        }
    }
}

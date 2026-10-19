// The disk map: where the space is now, measured with APFS private size and
// reconciled with what APFS says is in use. A whole-volume scan takes about a
// minute and reads metadata only, so it runs when asked, never by itself. The
// last one is kept, with when it was taken, and offered again until it's old.

import SwiftUI

struct MapView: View {
    @State private var map: DiskMap?
    @State private var measuredAt: Date?
    @State private var context = MapContext(growth24h: [])
    @State private var running = false
    @State private var started = Date()
    @State private var error: String?

    /// After this a map is said to be old, and measuring again is suggested.
    static let stale: TimeInterval = 30 * 60

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
                TimelineView(.periodic(from: .now, by: 60)) { ctx in
                    measureButton(old: isOld(at: ctx.date))
                }
            }
            if running {
                TimelineView(.periodic(from: .now, by: 1)) { ctx in
                    ProgressView("Measuring the whole disk… \(Int(ctx.date.timeIntervalSince(started))) s (about a minute)")
                        .progressViewStyle(.linear)
                }
            } else if let at = measuredAt {
                TimelineView(.periodic(from: .now, by: 60)) { ctx in
                    age(at, now: ctx.date)
                }
            }
            if let error {
                Label(error, systemImage: "exclamationmark.triangle").foregroundStyle(.orange)
            }
            if let m = map {
                Summary(map: m)
                MapExplorer(map: m, context: context)
            } else if !running {
                Spacer()
            }
        }
        .padding(20)
        .frame(minWidth: 720, minHeight: 640)
        .task { await loadKept() }
    }

    @ViewBuilder
    private func measureButton(old: Bool) -> some View {
        let b = Button(map == nil ? "Measure" : "Measure Again") { Task { await measure() } }
            .disabled(running)
            .keyboardShortcut(.defaultAction)
        if old || map == nil { b.buttonStyle(.borderedProminent) } else { b }
    }

    private func isOld(at now: Date) -> Bool {
        guard let at = measuredAt else { return false }
        return now.timeIntervalSince(at) > Self.stale
    }

    /// When the map was taken; past 30 minutes, a suggestion to measure again.
    @ViewBuilder
    private func age(_ at: Date, now: Date) -> some View {
        let ago = Format.duration(Int64(now.timeIntervalSince(at)))
        if now.timeIntervalSince(at) > Self.stale {
            Label("Measured \(Format.when(Int64(at.timeIntervalSince1970))), \(ago) ago. Files have come and gone since: measure again for the current picture.",
                  systemImage: "clock.arrow.circlepath")
                .font(.callout)
                .foregroundStyle(.orange)
        } else {
            Text("Measured \(Format.when(Int64(at.timeIntervalSince1970))), \(ago) ago.")
                .font(.callout)
                .foregroundStyle(.secondary)
        }
    }

    /// ~/Library/Caches/<bundle id>/disk-map.json: regenerable, and never
    /// leaves the Mac. Its modification time is when the map was taken.
    private static var keptURL: URL? {
        FileManager.default.urls(for: .cachesDirectory, in: .userDomainMask).first?
            .appendingPathComponent(Bundle.main.bundleIdentifier ?? "io.github.lgibelli.mydiskisfull")
            .appendingPathComponent("disk-map.json")
    }

    private func loadKept() async {
        await loadGrowth()
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

    private func measure() async {
        running = true
        started = Date()
        defer { running = false }
        await loadGrowth()
        do {
            let data = try await Core.run(["map", "--depth", "6", "--min-mb", "200", "--json"], timeout: 900)
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

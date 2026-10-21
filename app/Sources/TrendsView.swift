// Disk Usage: how full the disk is over time (% used), in layers by kind
// (macOS, apps, build output, caches, the Trash, snapshots…) from the
// whole-disk measurements, with where the trend goes (and how sure it is),
// the floor that raises an alert and the cleanups taken out of the trend;
// then what grew the most in the last day. What was written is on Disk Wear.

import Charts
import SwiftUI

struct TrendsPage: View {
    @ObservedObject var monitor: Monitor

    var body: some View {
        Page(title: "Disk Usage", subtitle: "How full the disk gets, and what changes it.") {
            if let s = monitor.status {
                FreeSpaceCard(status: s, series: monitor.detail?.series ?? [],
                              strata: monitor.detail?.strata ?? [], health: monitor.health)
                GrewList(rows: monitor.detail?.growth24h ?? [])
            } else {
                ProgressView().frame(maxWidth: .infinity, minHeight: 360)
            }
        }
    }
}

enum Span: String, CaseIterable, Identifiable {
    case day = "24 Hours", threeDays = "3 Days", twoWeeks = "2 Weeks", all = "All"
    var id: String { rawValue }
    var seconds: Double? {
        switch self {
        case .day: 86_400
        case .threeDays: 3 * 86_400
        case .twoWeeks: 14 * 86_400
        case .all: nil
        }
    }
}

private struct FreeSpaceCard: View {
    let status: Status
    let series: [[Int64]]
    let strata: [ForecastDetail.Strata]
    let health: Health
    @State private var span: Span = .twoWeeks
    @State private var hover: Date?

    private struct Point: Identifiable {
        let date: Date
        /// Percent of the disk used.
        let pct: Double
        var id: Double { date.timeIntervalSince1970 }
    }

    /// One layer at one time, from `lo` to `hi` percent of the disk.
    private struct Band: Identifiable {
        let layer: String
        let date: Date
        let lo: Double
        let hi: Double
        /// After the first measurement: a split, not a guess from a later one.
        let measured: Bool
        var id: String { "\(layer)-\(date.timeIntervalSince1970)" }
    }


    var body: some View {
        let points = history()
        let ahead = projection()
        let domain = yDomain(points, ahead)
        VStack(alignment: .leading, spacing: 14) {
            HStack(alignment: .firstTextBaseline) {
                VStack(alignment: .leading, spacing: 2) {
                    Text("Space used").font(.title3.weight(.semibold))
                    Text(Headline.title(status) + ". " + Headline.detail(status))
                        .font(.callout).foregroundStyle(.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                }
                Spacer(minLength: 16)
                Picker("Span", selection: $span) {
                    ForEach(Span.allCases) { Text($0.rawValue).tag($0) }
                }
                .pickerStyle(.segmented)
                .labelsHidden()
                .fixedSize()
            }
            if let p = status.forecast?.paces {
                PacesRow(paces: p)
            }
            if points.count < 2 {
                ContentUnavailableView("Not enough history yet", systemImage: "chart.xyaxis.line",
                                       description: Text("The chart starts once the watcher has a few samples. It takes one every 5 minutes."))
                    .frame(height: 300)
            } else {
                chart(points, ahead, domain)
                    .frame(height: 300)
                legend
            }
        }
        .card(padding: 22)
    }

    private var line: Color { health == .calm || health == .learning ? Theme.brand[0] : health.tint }

    @ViewBuilder
    private func chart(_ points: [Point], _ ahead: (mid: [Point], low: [Point], high: [Point]),
                       _ domain: ClosedRange<Double>) -> some View {
        Chart {
            if strata.isEmpty {
                ForEach(points) { p in
                    AreaMark(x: .value("Time", p.date), yStart: .value("Base", domain.lowerBound),
                             yEnd: .value("Used", p.pct), series: .value("Series", "Used"))
                        .foregroundStyle(LinearGradient(colors: [line.opacity(0.28), line.opacity(0.0)],
                                                        startPoint: .top, endPoint: .bottom))
                        .interpolationMethod(.monotone)
                }
            } else {
                // What it's made of, layer on layer, up to what's used.
                ForEach(bands(points)) { b in
                    AreaMark(x: .value("Time", b.date), yStart: .value("From", b.lo), yEnd: .value("To", b.hi),
                             series: .value("Layer", b.layer))
                        // Before the first measurement it's that split, faded.
                        .foregroundStyle(Layers.color(b.layer).opacity(b.measured ? 0.85 : 0.3))
                }
            }
            ForEach(points) { p in
                LineMark(x: .value("Time", p.date), y: .value("Used", p.pct), series: .value("Series", "Used"))
                    .foregroundStyle(strata.isEmpty ? line : Color.primary.opacity(0.75))
                    .lineStyle(StrokeStyle(lineWidth: strata.isEmpty ? 2.2 : 1.6, lineCap: .round, lineJoin: .round))
                    .interpolationMethod(.monotone)
            }
            // How sure the trend is: the plausible range of rates, as a band.
            ForEach(Array(zip(ahead.low, ahead.high).enumerated()), id: \.offset) { _, b in
                AreaMark(x: .value("Time", b.0.date), yStart: .value("Low", b.0.pct), yEnd: .value("High", b.1.pct),
                         series: .value("Series", "Band"))
                    .foregroundStyle(Color.orange.opacity(0.13))
            }
            ForEach(ahead.mid) { p in
                LineMark(x: .value("Time", p.date), y: .value("Used", p.pct), series: .value("Series", "Forecast"))
                    .foregroundStyle(Color.orange)
                    .lineStyle(StrokeStyle(lineWidth: 2, lineCap: .round, dash: [5, 5]))
            }
            if floorPct <= domain.upperBound {
                RuleMark(y: .value("Alert above", floorPct))
                    .foregroundStyle(Color.red.opacity(0.55))
                    .lineStyle(StrokeStyle(lineWidth: 1, dash: [3, 4]))
                    .annotation(position: .bottom, alignment: .leading) {
                        Text("Alert when less than \(Format.bytes(status.floor)) is free")
                            .font(.caption2.weight(.medium)).foregroundStyle(.red)
                    }
            }
            ForEach(cleanups(), id: \.ts) { step in
                PointMark(x: .value("Cleanup", Date(timeIntervalSince1970: TimeInterval(step.ts))),
                          y: .value("Used", pct(at: step.ts, points)))
                    .symbol {
                        Circle().fill(Color.green).frame(width: 9, height: 9)
                            .overlay(Circle().stroke(Theme.card, lineWidth: 2))
                    }
                    .annotation(position: .top, spacing: 4,
                                overflowResolution: .init(x: .fit(to: .chart), y: .fit(to: .chart))) {
                        // Only the big ones are labelled.
                        if step.freed >= 2_000_000_000 {
                            Text("+\(Format.bytes(step.freed))")
                                .font(.caption2.weight(.semibold)).foregroundStyle(.green)
                        }
                    }
            }
            if let hover, let p = nearest(hover, points) {
                RuleMark(x: .value("Time", p.date))
                    .foregroundStyle(Color.primary.opacity(0.25))
                    .lineStyle(StrokeStyle(lineWidth: 1))
                    .annotation(position: .top, spacing: 0,
                                overflowResolution: .init(x: .fit(to: .chart), y: .disabled)) {
                        Callout(point: p, total: status.total,
                                layers: strata.isEmpty ? [] : Layers.at(p.date, strata, used: Double(status.total) * p.pct / 100))
                    }
                PointMark(x: .value("Time", p.date), y: .value("Used", p.pct))
                    .symbolSize(60)
                    .foregroundStyle(line)
            }
        }
        .chartYScale(domain: domain)
        .chartYAxis {
            AxisMarks(position: .leading) { v in
                AxisGridLine(stroke: StrokeStyle(lineWidth: 0.5)).foregroundStyle(Color.primary.opacity(0.08))
                AxisValueLabel {
                    if let d = v.as(Double.self) {
                        Text(domain.upperBound - domain.lowerBound < 4
                             ? String(format: "%.1f%%", d) : "\(Int(d.rounded()))%")
                    }
                }
            }
        }
        .chartXAxis {
            AxisMarks { _ in
                AxisGridLine(stroke: StrokeStyle(lineWidth: 0.5)).foregroundStyle(Color.primary.opacity(0.05))
                AxisValueLabel()
            }
        }
        // What was free at the pointer, as it moves.
        .chartOverlay { proxy in
            GeometryReader { geo in
                Rectangle().fill(.clear).contentShape(Rectangle())
                    .onContinuousHover { phase in
                        switch phase {
                        case .active(let p):
                            guard let plot = proxy.plotFrame else { return }
                            hover = proxy.value(atX: p.x - geo[plot].origin.x, as: Date.self)
                        case .ended:
                            hover = nil
                        }
                    }
            }
        }
        .animation(.easeInOut(duration: 0.35), value: span)
    }

    /// What the disk did over the last hour, day and week: plain net
    /// changes, not paces projected to a day.
    private struct PacesRow: View {
        let paces: Forecast.Paces

        var body: some View {
            HStack(spacing: 28) {
                item("Last hour", paces.hour.map { $0 / 24 })
                item("Last 24 hours", paces.day)
                item("Last 7 days", paces.week.map { $0 * 7 })
                Spacer()
            }
            .help("How much the disk filled or freed, cleanups left out.")
        }

        @ViewBuilder
        private func item(_ label: String, _ bytes: Double?) -> some View {
            if let b = bytes {
                VStack(alignment: .leading, spacing: 2) {
                    Text(label.uppercased())
                        .font(.caption2.weight(.semibold))
                        .tracking(0.6)
                        .foregroundStyle(.secondary)
                    Text(Format.change(b))
                        .font(.system(.body, design: .rounded).weight(.semibold))
                        .monospacedDigit()
                        .foregroundStyle(abs(b) < 50_000_000 ? Color.secondary : b > 0 ? Color.orange : Color.green)
                }
            }
        }
    }

    private struct Callout: View {
        let point: Point
        let total: Int64
        /// The biggest layers then, with their bytes.
        let layers: [(String, Double)]

        var body: some View {
            VStack(alignment: .leading, spacing: 1) {
                Text("\(String(format: "%.1f", point.pct))% used")
                    .font(.callout.weight(.semibold)).monospacedDigit()
                Text("\(Format.bytes(Int64(Double(total) * (100 - point.pct) / 100))) free")
                    .font(.caption).monospacedDigit()
                ForEach(Array(layers.prefix(5).enumerated()), id: \.offset) { _, l in
                    HStack(spacing: 6) {
                        Circle().fill(Layers.color(l.0)).frame(width: 7, height: 7)
                        Text(Layers.title(l.0)).lineLimit(1)
                        Spacer(minLength: 8)
                        Text(Format.bytes(Int64(l.1))).monospacedDigit()
                    }
                    .font(.caption)
                    .frame(width: 210)
                }
                Text(point.date.formatted(.dateTime.weekday(.abbreviated).hour().minute()))
                    .font(.caption).foregroundStyle(.secondary)
            }
            .padding(.horizontal, 10)
            .padding(.vertical, 6)
            .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 8, style: .continuous))
            .overlay(RoundedRectangle(cornerRadius: 8, style: .continuous).strokeBorder(Theme.hairline))
        }
    }

    private var legend: some View {
        VStack(alignment: .leading, spacing: 10) {
            if !strata.isEmpty {
                LazyVGrid(columns: [GridItem(.adaptive(minimum: 150), alignment: .leading)], alignment: .leading,
                          spacing: 6) {
                    ForEach(Layers.present(strata), id: \.self) { l in
                        key(RoundedRectangle(cornerRadius: 2).fill(Layers.color(l)).frame(width: 12, height: 9),
                            Layers.title(l))
                    }
                }
                Text(strataNote)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
            keys
        }
        .font(.caption)
    }

    /// Where the layers come from.
    private var strataNote: String {
        let n = strata.count
        let last = strata.last.map { Format.ago($0.ts) } ?? ""
        return "The layers come from \(n == 1 ? "one whole-disk measurement" : "\(n) whole-disk measurements"), the last \(last): between them the split is estimated, and before the first it's that one's, faded. The line on top is what was used."
    }

    private var keys: some View {
        HStack(spacing: 18) {
            key(Circle().fill(strata.isEmpty ? line : Color.primary.opacity(0.75)).frame(width: 8, height: 8), "Space used")
            if status.forecast?.rate.map({ $0.perDay > 0 }) == true {
                key(Capsule().fill(Color.orange).frame(width: 14, height: 3), "Where the trend goes")
                key(RoundedRectangle(cornerRadius: 2).fill(Color.orange.opacity(0.2)).frame(width: 14, height: 8),
                    "Plausible range")
            }
            if !cleanups().isEmpty {
                key(Circle().fill(Color.green).frame(width: 8, height: 8), "Cleanup, left out of the trend")
            }
            Spacer()
            if let r = status.forecast?.rate {
                Text("\(Format.days(Double(r.window) / 86_400)) of history · \(r.points) samples")
                    .foregroundStyle(.secondary)
            }
        }
    }

    private func key(_ mark: some View, _ text: String) -> some View {
        HStack(spacing: 6) {
            mark
            Text(text).foregroundStyle(.secondary)
        }
    }

    // MARK: - Data

    private func history() -> [Point] {
        let cutoff = span.seconds.map { Date().timeIntervalSince1970 - $0 } ?? 0
        return series.compactMap { p in
            guard p.count == 2, Double(p[0]) >= cutoff else { return nil }
            return Point(date: Date(timeIntervalSince1970: TimeInterval(p[0])), pct: used(p[1]))
        }
    }

    /// Percent of the disk used when `avail` is free.
    private func used(_ avail: Int64) -> Double {
        guard status.total > 0 else { return 0 }
        return min(100, max(0, Double(status.total - avail) / Double(status.total) * 100))
    }

    /// Each point's layers, stacked: the measured split at that time (or
    /// between two measurements, in between) times what was used. At most
    /// 200 points, so the chart stays quick.
    private func bands(_ points: [Point]) -> [Band] {
        let step = max(1, points.count / 200)
        let shown = stride(from: 0, to: points.count, by: step).map { points[$0] } + (points.count % step == 0 ? [] : [points[points.count - 1]])
        var out: [Band] = []
        let first = strata.first.map { Date(timeIntervalSince1970: TimeInterval($0.ts)) } ?? .distantFuture
        for p in shown {
            var lo = 0.0
            for (layer, share) in Layers.shares(at: p.date, strata) {
                let hi = lo + share * p.pct
                out.append(Band(layer: layer, date: p.date, lo: lo, hi: hi, measured: p.date >= first))
                lo = hi
            }
        }
        return out
    }

    /// Where the alert floor is, in percent used.
    private var floorPct: Double { used(status.floor) }

    /// Where the trend goes, a third of the span ahead or to the day it's
    /// full, with the plausible range of rates around it.
    private func projection() -> (mid: [Point], low: [Point], high: [Point]) {
        guard let f = status.forecast, let r = f.rate, r.perDay > 0 else { return ([], [], []) }
        let ahead = (span.seconds ?? 14 * 86_400) / 3
        let secs = min(ahead, (f.fullIn?.days ?? .infinity) * 86_400)
        let start = Date(timeIntervalSince1970: TimeInterval(f.at))
        let avail = Double(f.avail)
        func at(_ t: Double, _ rate: Double) -> Point {
            Point(date: start.addingTimeInterval(t), pct: used(Int64(max(0, avail - rate * t / 86_400))))
        }
        let ts = stride(from: 0.0, through: secs, by: max(secs / 12, 1)).map { $0 }
        return (ts.map { at($0, r.perDay) }, ts.map { at($0, max(r.high, r.perDay)) }, ts.map { at($0, max(0, r.low)) })
    }

    private func steps() -> [Step] {
        let cutoff = span.seconds.map { Int64(Date().timeIntervalSince1970 - $0) } ?? 0
        return (status.forecast?.steps ?? []).filter { $0.ts >= cutoff }
    }

    /// Cleanups within an hour of each other as one, so their labels
    /// don't pile up.
    private func cleanups() -> [Step] {
        var out: [Step] = []
        for s in steps().sorted(by: { $0.ts < $1.ts }) {
            if let last = out.last, s.ts - last.ts < 3_600 {
                out[out.count - 1] = Step(ts: s.ts, freed: last.freed + s.freed)
            } else {
                out.append(s)
            }
        }
        return out
    }

    private func pct(at ts: Int64, _ points: [Point]) -> Double {
        let t = Date(timeIntervalSince1970: TimeInterval(ts))
        return nearest(t, points)?.pct ?? 0
    }

    private func nearest(_ t: Date, _ points: [Point]) -> Point? {
        points.min { abs($0.date.timeIntervalSince(t)) < abs($1.date.timeIntervalSince(t)) }
    }

    /// Fitted to what's shown, not from zero: a few GB of change on a disk
    /// of hundreds would otherwise be a flat line.
    private func yDomain(_ points: [Point], _ ahead: (mid: [Point], low: [Point], high: [Point])) -> ClosedRange<Double> {
        let ys = points.map(\.pct) + ahead.mid.map(\.pct) + ahead.high.map(\.pct) + ahead.low.map(\.pct)
        // In layers, from the bottom: every layer shows.
        if !strata.isEmpty {
            return 0...min(100, ((ys.max() ?? 90) / 10 + 0.5).rounded(.up) * 10)
        }
        guard let lo = ys.min(), let hi = ys.max() else { return 0...100 }
        let pad = max((hi - lo) * 0.12, 0.5)
        return max(0, lo - pad)...min(100, hi + pad)
    }
}

private struct GrewList: View {
    let rows: [ForecastDetail.Growth]

    var body: some View {
        let shown = rows.filter { $0.bytes > 0 }.prefix(8)
        VStack(alignment: .leading, spacing: 14) {
            SectionHeader(title: "Grew the most", subtitle: "Folders, the last 24 hours")
            if shown.isEmpty {
                Text("Nothing grew much.").font(.callout).foregroundStyle(.secondary)
            }
            ForEach(Array(shown.enumerated()), id: \.offset) { _, g in
                BarRow(name: URL(fileURLWithPath: g.owner).lastPathComponent,
                       detail: Format.path(URL(fileURLWithPath: g.owner).deletingLastPathComponent().path),
                       value: g.bytes, largest: shown.first?.bytes ?? 1, color: .orange,
                       help: Format.path(g.owner))
            }
        }
        .frame(maxHeight: .infinity, alignment: .top)
        .card()
    }
}

/// The layers of the disk over time: their order (macOS at the bottom, the
/// Trash near the top), names, colours, and the split at a time.
enum Layers {
    static let order = ["macos", "unseen", "apps", "dev", "build", "vms", "cloud", "appdata", "mail",
                        "media", "files", "caches", "trash", "other"]

    static func title(_ id: String) -> String {
        switch id {
        case "macos": "macOS"
        case "unseen": "Snapshots and system data"
        case "apps": "Apps"
        case "dev": "Developer tools"
        case "build": "Build output"
        case "vms": "Virtual machines"
        case "cloud": "Cloud copies"
        case "appdata": "App data"
        case "mail": "Mail and Messages"
        case "media": "Photos, music and video"
        case "files": "Your files"
        case "caches": "Caches"
        case "trash": "Trash"
        default: "Other"
        }
    }

    static func color(_ id: String) -> Color {
        switch id {
        case "unseen": Color(red: 0.45, green: 0.48, blue: 0.55)
        case "trash": .brown
        case "other": Color(white: 0.72)
        default: SpaceText.color(id)
        }
    }

    /// The layers any measurement has, in order.
    static func present(_ strata: [ForecastDetail.Strata]) -> [String] {
        let ids = Set(strata.flatMap { $0.layers.keys })
        return order.filter(ids.contains) + ids.subtracting(order).sorted()
    }

    /// Each layer's share of what was used at `date`, in order: the nearest
    /// measurement's before the first and after the last, in between the
    /// two around it, weighed by how near each is.
    static func shares(at date: Date, _ strata: [ForecastDetail.Strata]) -> [(String, Double)] {
        let ids = present(strata)
        func split(_ s: ForecastDetail.Strata) -> [Double] {
            let total = Double(max(s.layers.values.reduce(0, +), 1))
            return ids.map { Double(s.layers[$0] ?? 0) / total }
        }
        let t = Int64(date.timeIntervalSince1970)
        guard let first = strata.first, let last = strata.last else { return [] }
        let mix: [Double]
        if t <= first.ts {
            mix = split(first)
        } else if t >= last.ts {
            mix = split(last)
        } else if let i = strata.lastIndex(where: { $0.ts <= t }), i + 1 < strata.count {
            let a = strata[i], b = strata[i + 1]
            let w = Double(t - a.ts) / Double(max(b.ts - a.ts, 1))
            mix = zip(split(a), split(b)).map { $0 * (1 - w) + $1 * w }
        } else {
            mix = split(last)
        }
        return Array(zip(ids, mix))
    }

    /// The layers at `date`, biggest first, in bytes of what was `used`.
    static func at(_ date: Date, _ strata: [ForecastDetail.Strata], used: Double) -> [(String, Double)] {
        shares(at: date, strata).map { ($0.0, $0.1 * used) }.sorted { $0.1 > $1.1 }
    }
}

// Trends: free space over time with where the trend goes (and how sure it
// is), the floor that raises an alert, the cleanups taken out of the trend;
// then what grew and what wrote the most in the last day.

import Charts
import SwiftUI

struct TrendsPage: View {
    @ObservedObject var monitor: Monitor

    var body: some View {
        Page(title: "Trends", subtitle: "How free space changes, and what changes it.") {
            if let s = monitor.status {
                FreeSpaceCard(status: s, series: monitor.detail?.series ?? [], health: monitor.health)
                HStack(alignment: .top, spacing: 16) {
                    GrewList(rows: monitor.detail?.growth24h ?? [])
                    #if APP_STORE
                    FullVersionNote(missing: .writers, title: "Wrote the most")
                        .card()
                    #else
                    WroteList(rows: monitor.detail?.writes24h ?? [])
                    #endif
                }
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
    let health: Health
    @State private var span: Span = .threeDays
    @State private var hover: Date?

    private struct Point: Identifiable {
        let date: Date
        let gb: Double
        var id: Double { date.timeIntervalSince1970 }
    }

    var body: some View {
        let points = history()
        let ahead = projection()
        let domain = yDomain(points, ahead)
        VStack(alignment: .leading, spacing: 14) {
            HStack(alignment: .firstTextBaseline) {
                VStack(alignment: .leading, spacing: 2) {
                    Text("Free space").font(.title3.weight(.semibold))
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
            ForEach(points) { p in
                AreaMark(x: .value("Time", p.date), yStart: .value("Base", domain.lowerBound),
                         yEnd: .value("Free", p.gb), series: .value("Series", "Free"))
                    .foregroundStyle(LinearGradient(colors: [line.opacity(0.28), line.opacity(0.0)],
                                                    startPoint: .top, endPoint: .bottom))
                    .interpolationMethod(.monotone)
                LineMark(x: .value("Time", p.date), y: .value("Free", p.gb), series: .value("Series", "Free"))
                    .foregroundStyle(line)
                    .lineStyle(StrokeStyle(lineWidth: 2.2, lineCap: .round, lineJoin: .round))
                    .interpolationMethod(.monotone)
            }
            // How sure the trend is: the plausible range of rates, as a band.
            ForEach(Array(zip(ahead.low, ahead.high).enumerated()), id: \.offset) { _, b in
                AreaMark(x: .value("Time", b.0.date), yStart: .value("Low", b.0.gb), yEnd: .value("High", b.1.gb),
                         series: .value("Series", "Band"))
                    .foregroundStyle(Color.orange.opacity(0.13))
            }
            ForEach(ahead.mid) { p in
                LineMark(x: .value("Time", p.date), y: .value("Free", p.gb), series: .value("Series", "Forecast"))
                    .foregroundStyle(Color.orange)
                    .lineStyle(StrokeStyle(lineWidth: 2, lineCap: .round, dash: [5, 5]))
            }
            if Double(status.floor) / 1e9 >= domain.lowerBound {
                RuleMark(y: .value("Alert below", Double(status.floor) / 1e9))
                    .foregroundStyle(Color.red.opacity(0.55))
                    .lineStyle(StrokeStyle(lineWidth: 1, dash: [3, 4]))
                    .annotation(position: .top, alignment: .leading) {
                        Text("Alert below \(Format.bytes(status.floor))")
                            .font(.caption2.weight(.medium)).foregroundStyle(.red)
                    }
            }
            ForEach(cleanups(), id: \.ts) { step in
                PointMark(x: .value("Cleanup", Date(timeIntervalSince1970: TimeInterval(step.ts))),
                          y: .value("Free", gb(at: step.ts, points)))
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
                        Callout(point: p)
                    }
                PointMark(x: .value("Time", p.date), y: .value("Free", p.gb))
                    .symbolSize(60)
                    .foregroundStyle(line)
            }
        }
        .chartYScale(domain: domain)
        .chartYAxis {
            AxisMarks(position: .leading) { v in
                AxisGridLine(stroke: StrokeStyle(lineWidth: 0.5)).foregroundStyle(Color.primary.opacity(0.08))
                AxisValueLabel { if let d = v.as(Double.self) { Text("\(Int(d.rounded())) GB") } }
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

    private struct Callout: View {
        let point: Point

        var body: some View {
            VStack(alignment: .leading, spacing: 1) {
                Text("\(String(format: "%.1f", point.gb)) GB free")
                    .font(.callout.weight(.semibold)).monospacedDigit()
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
        HStack(spacing: 18) {
            key(Circle().fill(line).frame(width: 8, height: 8), "Free space")
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
                Text("\(Format.window(r.window)) trend · \(r.points) samples")
                    .foregroundStyle(.secondary)
            }
        }
        .font(.caption)
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
            return Point(date: Date(timeIntervalSince1970: TimeInterval(p[0])), gb: Double(p[1]) / 1e9)
        }
    }

    /// Where the trend goes, a third of the span ahead or to the day it's
    /// full, with the plausible range of rates around it.
    private func projection() -> (mid: [Point], low: [Point], high: [Point]) {
        guard let f = status.forecast, let r = f.rate, r.perDay > 0 else { return ([], [], []) }
        let ahead = (span.seconds ?? 14 * 86_400) / 3
        let secs = min(ahead, (f.fullIn?.days ?? .infinity) * 86_400)
        let start = Date(timeIntervalSince1970: TimeInterval(f.at))
        let avail = Double(f.avail)
        func at(_ t: Double, _ rate: Double) -> Point {
            Point(date: start.addingTimeInterval(t), gb: max(0, avail - rate * t / 86_400) / 1e9)
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

    private func gb(at ts: Int64, _ points: [Point]) -> Double {
        let t = Date(timeIntervalSince1970: TimeInterval(ts))
        return nearest(t, points)?.gb ?? 0
    }

    private func nearest(_ t: Date, _ points: [Point]) -> Point? {
        points.min { abs($0.date.timeIntervalSince(t)) < abs($1.date.timeIntervalSince(t)) }
    }

    /// Fitted to what's shown, not from zero: a few GB of change on a disk
    /// with hundreds free would otherwise be a flat line.
    private func yDomain(_ points: [Point], _ ahead: (mid: [Point], low: [Point], high: [Point])) -> ClosedRange<Double> {
        let ys = points.map(\.gb) + ahead.mid.map(\.gb) + ahead.high.map(\.gb) + ahead.low.map(\.gb)
        guard let lo = ys.min(), let hi = ys.max() else { return 0...1 }
        let pad = max((hi - lo) * 0.12, 1)
        return max(0, lo - pad)...(hi + pad)
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

#if !APP_STORE
private struct WroteList: View {
    let rows: [ForecastDetail.Writer]

    var body: some View {
        let shown = rows.prefix(8)
        VStack(alignment: .leading, spacing: 14) {
            SectionHeader(title: "Wrote the most", subtitle: "Apps, the last 24 hours")
            if shown.isEmpty {
                Text("Nothing yet.").font(.callout).foregroundStyle(.secondary)
            }
            ForEach(Array(shown.enumerated()), id: \.offset) { _, w in
                BarRow(name: w.name, value: w.bytes, largest: shown.first?.bytes ?? 1, color: .purple)
            }
            Text("Writing isn't filling: an app can rewrite the same files all day. Heavy writing wears the SSD.")
                .font(.caption).foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
        }
        .frame(maxHeight: .infinity, alignment: .top)
        .card()
    }
}
#endif

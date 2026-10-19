// The overview: how full the disk is, where the trend goes, what needs
// attention, and what grew and wrote the most in the last day.

import Charts
import SwiftUI

struct OverviewView: View {
    @ObservedObject var monitor: Monitor
    @State private var detail: ForecastDetail?
    @State private var error: String?
    @State private var span: Span = .threeDays

    enum Span: String, CaseIterable, Identifiable {
        case day = "1 day", threeDays = "3 days", twoWeeks = "14 days", all = "All"
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

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 18) {
                if let s = monitor.status {
                    Header(status: s)
                    Attention(monitor: monitor)
                    chart(s)
                    if let d = detail {
                        HStack(alignment: .top, spacing: 24) {
                            TopList(title: "Grew the most, last 24 hours",
                                    rows: d.growth24h.filter { $0.bytes > 0 }.prefix(8)
                                        .map { (Format.path($0.owner), $0.bytes) })
                            TopList(title: "Wrote the most, last 24 hours",
                                    rows: d.writes24h.prefix(8).map { ($0.name, $0.bytes) })
                        }
                    }
                } else if let e = monitor.statusError ?? error {
                    Label(e, systemImage: "exclamationmark.triangle")
                } else {
                    ProgressView()
                }
            }
            .padding(20)
            .frame(maxWidth: .infinity, alignment: .leading)
        }
        .frame(minWidth: 620, minHeight: 520)
        .task(id: monitor.lastRefresh) { await load() }
    }

    private func load() async {
        do {
            detail = try await Core.json(ForecastDetail.self, ["forecast", "--series"])
            error = nil
        } catch {
            self.error = error.localizedDescription
        }
    }

    // MARK: - The chart

    private struct Point: Identifiable {
        let date: Date
        let gb: Double
        let series: String
        var id: String { "\(series)\(date.timeIntervalSince1970)" }
    }

    @ViewBuilder
    private func chart(_ s: Status) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack {
                Text("Free space").font(.headline)
                Spacer()
                Picker("", selection: $span) {
                    ForEach(Span.allCases) { Text($0.rawValue).tag($0) }
                }
                .pickerStyle(.segmented)
                .labelsHidden()
                .frame(width: 280)
            }
            let points = history()
            if points.count < 2 {
                Text("The chart starts once the watcher has a few samples.")
                    .foregroundStyle(.secondary)
                    .frame(maxWidth: .infinity, minHeight: 120)
            } else {
                Chart {
                    ForEach(points + projection(s)) { p in
                        LineMark(x: .value("Time", p.date), y: .value("GB free", p.gb))
                            .foregroundStyle(by: .value("Series", p.series))
                            .lineStyle(p.series == "Forecast" ? StrokeStyle(lineWidth: 1.5, dash: [5, 4])
                                                              : StrokeStyle(lineWidth: 2))
                    }
                    ForEach(steps(s), id: \.ts) { step in
                        RuleMark(x: .value("Cleanup", Date(timeIntervalSince1970: TimeInterval(step.ts))))
                            .foregroundStyle(.green.opacity(0.4))
                            .annotation(position: .top, alignment: .leading) {
                                Text("+\(Format.bytes(step.freed))").font(.caption2).foregroundStyle(.green)
                            }
                    }
                }
                .chartForegroundStyleScale(["Free": Color.accentColor, "Forecast": Color.orange])
                .chartYAxisLabel("GB free")
                .frame(height: 220)
                if let r = s.forecast?.rate {
                    Text(rateLine(r)).font(.caption).foregroundStyle(.secondary)
                }
            }
        }
    }

    private func history() -> [Point] {
        guard let d = detail else { return [] }
        let cutoff = span.seconds.map { Date().timeIntervalSince1970 - $0 } ?? 0
        return (d.series ?? []).compactMap { p in
            guard p.count == 2, Double(p[0]) >= cutoff else { return nil }
            return Point(date: Date(timeIntervalSince1970: TimeInterval(p[0])), gb: Double(p[1]) / 1e9,
                         series: "Free")
        }
    }

    /// Where the trend goes: half the span ahead, or to the day it's full.
    private func projection(_ s: Status) -> [Point] {
        guard let f = s.forecast, let r = f.rate, r.perDay > 0 else { return [] }
        let ahead = (span.seconds ?? 14 * 86_400) / 2
        let secs = min(ahead, (f.fullIn?.days ?? .infinity) * 86_400)
        let start = Date(timeIntervalSince1970: TimeInterval(f.at))
        let end = Double(f.avail) - r.perDay * secs / 86_400
        return [
            Point(date: start, gb: Double(f.avail) / 1e9, series: "Forecast"),
            Point(date: start.addingTimeInterval(secs), gb: max(0, end) / 1e9, series: "Forecast"),
        ]
    }

    private func steps(_ s: Status) -> [Step] {
        let cutoff = span.seconds.map { Int64(Date().timeIntervalSince1970 - $0) } ?? 0
        return (s.forecast?.steps ?? []).filter { $0.ts >= cutoff }
    }

    private func rateLine(_ r: Rate) -> String {
        let pace = r.perDay > 0 ? "Filling at \(Format.bytes(r.perDay)) a day"
                                : "Not filling (\(Format.bytes(-r.perDay)) a day freed)"
        return "\(pace), the \(Format.window(r.window)) trend from \(r.points) samples; "
            + "plausibly \(Format.bytes(max(0, r.low)))–\(Format.bytes(max(0, r.high))) a day. "
            + "Cleanups are taken out of the trend."
    }
}

/// How full, and when it's full.
private struct Header: View {
    let status: Status

    var body: some View {
        let s = status
        let used = Double(s.total - s.avail)
        VStack(alignment: .leading, spacing: 8) {
            HStack(alignment: .firstTextBaseline) {
                Text("\(Format.bytes(s.avail)) free").font(.title.bold())
                Text("of \(Format.bytes(s.total))").foregroundStyle(.secondary)
                Spacer()
                if let first = s.history.first {
                    Text("Watching since \(Format.when(first)) · \(s.history.samples) samples")
                        .font(.caption).foregroundStyle(.secondary)
                }
            }
            Gauge(value: used, in: 0...Double(max(s.total, 1))) {}
                .gaugeStyle(.linearCapacity)
                .tint(s.alert >= .within3Days ? .red : (s.alert > .none ? .orange : .accentColor))
            Text(sentence).font(.callout)
        }
    }

    private var sentence: String {
        let s = status
        if let a = s.alertSentence { return a }
        guard let f = s.forecast else { return "No samples yet: the watcher takes one every 5 minutes." }
        guard let r = f.rate else {
            return "Collecting history: a forecast needs 8 samples covering 3 hours."
        }
        if r.perDay <= 0 { return "Not filling over the \(Format.window(r.window)) trend." }
        if let full = f.fullIn {
            return "At \(Format.bytes(r.perDay)) a day, the disk is full in about \(Format.days(full.days)), around \(Format.date(at: f.at, inDays: full.days))."
        }
        return "Filling at \(Format.bytes(r.perDay)) a day."
    }
}

/// Incidents, undone tweaks and watcher problems, each with what to do.
private struct Attention: View {
    @ObservedObject var monitor: Monitor

    var body: some View {
        let s = monitor.status
        VStack(alignment: .leading, spacing: 10) {
            if let p = monitor.watcherProblem {
                Card(icon: "pause.circle", title: p, text: monitor.watcher.caption) {
                    Button("Start Watching") { Task { await monitor.watcher.start(); await monitor.refresh() } }
                }
            }
            if monitor.lacksFullDiskAccess {
                Card(icon: "lock", title: "The watcher can't see into other apps' folders",
                     text: "macOS keeps it out without Full Disk Access, so what grows there goes unseen.") {
                    Button("Open Privacy Settings…") { Privacy.openSettings() }
                }
            }
            ForEach(s?.incidents ?? []) { e in
                Card(icon: "flame", title: e.incident.kind == .growth ? "Keeps growing" : "Keeps rewriting the disk",
                     text: e.sentence) {
                    FixButton(fix: e.fix, line: e.fixLine, monitor: monitor)
                }
            }
            ForEach(monitor.undone) { t in
                Card(icon: "arrow.uturn.backward", title: "Undone since: \(t.title)",
                     text: "Something changed its setting back, probably an update.") {
                    Button("Apply Again…") { monitor.apply(t.id) }
                }
            }
        }
    }
}

private struct FixButton: View {
    let fix: Fix
    let line: String
    @ObservedObject var monitor: Monitor

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            switch fix {
            case .tweak(let id):
                Button("Fix: \(monitor.tweak(id)?.title ?? id)…") { monitor.apply(id) }
            case .run(let command, let note):
                Text(note).font(.caption).foregroundStyle(.secondary)
                HStack {
                    Text(command).font(.caption.monospaced()).textSelection(.enabled)
                    Button("Copy") { monitor.copy(command) }.controlSize(.small)
                }
            case .steps, .leave:
                Text(line).font(.caption).foregroundStyle(.secondary)
            }
        }
    }
}

private struct Card<Actions: View>: View {
    let icon: String
    let title: String
    let text: String
    @ViewBuilder let actions: Actions

    var body: some View {
        HStack(alignment: .top, spacing: 12) {
            Image(systemName: icon).font(.title2).foregroundStyle(.orange).frame(width: 28)
            VStack(alignment: .leading, spacing: 6) {
                Text(title).font(.headline)
                Text(text).fixedSize(horizontal: false, vertical: true)
                actions
            }
            Spacer(minLength: 0)
        }
        .padding(12)
        .background(Color.primary.opacity(0.045), in: RoundedRectangle(cornerRadius: 8))
    }
}

private struct TopList: View {
    let title: String
    let rows: [(String, Int64)]

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(title).font(.headline)
            if rows.isEmpty {
                Text("Nothing yet.").foregroundStyle(.secondary)
            }
            ForEach(Array(rows.enumerated()), id: \.offset) { _, row in
                HStack {
                    Text(row.0).lineLimit(1).truncationMode(.middle)
                    Spacer()
                    Text(Format.bytes(row.1)).monospacedDigit().foregroundStyle(.secondary)
                }
                .font(.callout)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}

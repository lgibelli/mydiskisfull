// Disk wear: how much is written each day, and which apps write the most.
// Writing doesn't fill the disk (an app can rewrite the same files all
// day), but an SSD takes only so much writing in its life, so a heavy
// writer wears it out. From the watcher's per-process writes (`mdif
// forecast --series --json`: `writes_daily`, `writes_24h`); an app that
// keeps rewriting the disk is also an incident, with its fix, on Overview.

import Charts
import SwiftUI

struct WearPage: View {
    @ObservedObject var monitor: Monitor

    var body: some View {
        Page(title: "Disk Wear", subtitle: "Which apps write the most to the disk.") {
            #if APP_STORE
            FullVersionNote(missing: .writers, title: "Wrote the most")
                .card()
            #else
            if let rows = monitor.detail?.writes24h {
                WearSummary(rows: rows, total: monitor.status?.total)
                DailyCard(days: monitor.detail?.writesDaily ?? [], total: monitor.status?.total ?? 0)
                WritersCard(rows: rows)
            } else {
                ProgressView().frame(maxWidth: .infinity, minHeight: 360)
            }
            #endif
        }
    }
}

#if !APP_STORE
/// How much was written in a day, against the disk's own size.
private struct WearSummary: View {
    let rows: [ForecastDetail.Writer]
    let total: Int64?

    var body: some View {
        let written = rows.reduce(0) { $0 + $1.bytes }
        HStack(alignment: .top, spacing: 16) {
            IconTile(symbol: "flame.fill", color: written > (total ?? .max) ? .red : .orange, size: 40)
            VStack(alignment: .leading, spacing: 4) {
                Text("\(Format.bytes(written)) written in the last 24 hours")
                    .font(.title2.weight(.bold))
                if let total, total > 0 {
                    let times = Double(written) / Double(total)
                    Text(times >= 0.1
                         ? "That's \(String(format: "%.1f", times)) times the whole disk, in a day."
                         : "A small share of the disk's size: nothing to worry about.")
                        .font(.callout).foregroundStyle(.secondary)
                }
                Text("Writing doesn't fill the disk: an app can rewrite the same files all day. But an SSD can take only so much writing in its life, so an app that writes this much wears it out. An app that keeps rewriting the disk shows on Overview, with its fix.")
                    .font(.caption).foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
                    .padding(.top, 2)
            }
            Spacer(minLength: 0)
        }
        .card(padding: 20, tint: .orange)
    }
}

/// What was written each day, the last 30, stacked by the biggest writers
/// of those days (the rest as Other), with the day's own biggest under the
/// pointer.
private struct DailyCard: View {
    let days: [ForecastDetail.DayWrites]
    /// The disk's size: a day that writes as much is marked.
    let total: Int64
    @State private var selected: Date?

    private static let shown = 30

    var body: some View {
        let now = Date()
        let cal = Calendar.current
        let earliest = cal.startOfDay(for: now.addingTimeInterval(-Double(Self.shown - 1) * 86_400))
        let rows = days.filter { Double($0.day) >= earliest.timeIntervalSince1970 - 3_600 }
        // From the first day there is, a week at least: a few days of
        // history aren't squeezed into a month.
        let first = rows.first.map(date) ?? earliest
        let start = min(max(earliest, cal.startOfDay(for: first)),
                        cal.startOfDay(for: now.addingTimeInterval(-6 * 86_400)))
        let largest = rows.map(\.bytes).max() ?? 0
        let full = rows.filter { $0.day < Int64(Calendar.current.startOfDay(for: now).timeIntervalSince1970) }
        VStack(alignment: .leading, spacing: 14) {
            SectionHeader(title: "Written each day", subtitle: subtitle(full, all: rows))
                .help("The last \(Self.shown) days at most, as the watcher counted them.")
            if rows.isEmpty {
                Text("Nothing yet: the watcher counts writes from its next samples.")
                    .font(.callout).foregroundStyle(.secondary)
            } else {
                let names = Self.writers(rows)
                Chart {
                    // Each day's biggest writers in their colours, the rest
                    // as Other, stacked; today's paler, as it isn't over.
                    ForEach(rows, id: \.day) { d in
                        let today = Calendar.current.isDateInToday(date(d))
                        ForEach(Self.parts(d, names), id: \.0) { part in
                            BarMark(x: .value("Day", date(d), unit: .day), y: .value("Written", Double(part.1) / 1e9))
                                .foregroundStyle(by: .value("App", part.0))
                                .opacity(today ? 0.5 : 1)
                        }
                    }
                    if total > 0, Double(largest) >= Double(total) * 0.3 {
                        RuleMark(y: .value("The whole disk", Double(total) / 1e9))
                            .foregroundStyle(Color.red.opacity(0.5))
                            .lineStyle(StrokeStyle(lineWidth: 1, dash: [3, 4]))
                            .annotation(position: .top, alignment: .leading) {
                                Text("The disk's whole size, \(Format.bytes(total))")
                                    .font(.caption2.weight(.medium)).foregroundStyle(.red)
                            }
                    }
                    if let selected, let d = day(selected, rows) {
                        RuleMark(x: .value("Day", date(d), unit: .day))
                            .foregroundStyle(Color.primary.opacity(0.08))
                            .annotation(position: .top, spacing: 0,
                                        overflowResolution: .init(x: .fit(to: .chart), y: .disabled)) {
                                DayCallout(day: d, date: date(d))
                            }
                    }
                }
                .chartForegroundStyleScale(domain: names + [Self.other],
                                           range: Array(Self.palette.prefix(names.count)) + [Color(white: 0.7)])
                .chartLegend(position: .bottom, alignment: .leading, spacing: 12)
                .chartXScale(domain: start...Calendar.current.startOfDay(for: now).addingTimeInterval(86_400))
                .chartXSelection(value: $selected)
                .chartXAxis {
                    AxisMarks(values: .stride(by: .day, count: rows.count > 14 ? 5 : 1)) { _ in
                        AxisGridLine(stroke: StrokeStyle(lineWidth: 0.5)).foregroundStyle(Color.primary.opacity(0.05))
                        AxisValueLabel(format: .dateTime.day().month(.abbreviated))
                    }
                }
                .chartYAxis {
                    AxisMarks(position: .leading) { v in
                        AxisGridLine(stroke: StrokeStyle(lineWidth: 0.5)).foregroundStyle(Color.primary.opacity(0.08))
                        AxisValueLabel { if let g = v.as(Double.self) { Text(Format.bytes(Int64(g * 1e9))) } }
                    }
                }
                .frame(height: 240)
                Text("Today so far is the paler bar. Point at a day to see what wrote the most that day.")
                    .font(.caption).foregroundStyle(.secondary)
            }
        }
        .card()
    }

    private static let other = "Other"
    private static let palette: [Color] = [.purple, .red, .orange, .blue, .teal]

    /// The biggest writers over the days shown, each with a colour.
    private static func writers(_ rows: [ForecastDetail.DayWrites]) -> [String] {
        var sum: [String: Int64] = [:]
        for d in rows { for w in d.top { sum[w.name, default: 0] += w.bytes } }
        return sum.sorted { $0.value > $1.value }.prefix(palette.count).map(\.key)
    }

    /// A day's bar in parts: those writers, then all the rest.
    private static func parts(_ d: ForecastDetail.DayWrites, _ names: [String]) -> [(String, Int64)] {
        let mine = names.map { n in (n, d.top.first { $0.name == n }?.bytes ?? 0) }.filter { $0.1 > 0 }
        let rest = d.bytes - mine.reduce(0) { $0 + $1.1 }
        return mine + (rest > 0 ? [(other, rest)] : [])
    }

    private func date(_ d: ForecastDetail.DayWrites) -> Date {
        Date(timeIntervalSince1970: TimeInterval(d.day))
    }

    private func day(_ t: Date, _ rows: [ForecastDetail.DayWrites]) -> ForecastDetail.DayWrites? {
        let s = Int64(t.timeIntervalSince1970)
        return rows.last { $0.day <= s && s < $0.day + 90_000 }
    }

    /// The average of the whole days; the most of any, today's so far too.
    private func subtitle(_ full: [ForecastDetail.DayWrites], all: [ForecastDetail.DayWrites]) -> String {
        guard !full.isEmpty else { return "The last \(Self.shown) days." }
        let mean = full.reduce(0) { $0 + $1.bytes } / Int64(full.count)
        var t = "On average \(Format.bytes(mean)) a day"
        if let most = all.max(by: { $0.bytes < $1.bytes }), all.count > 1 {
            t += Calendar.current.isDateInToday(date(most))
                ? "; the most today, \(Format.bytes(most.bytes)) so far"
                : "; the most \(Format.bytes(most.bytes)), on \(date(most).formatted(.dateTime.weekday(.wide).day().month(.abbreviated)))"
        }
        return t + "."
    }

    private struct DayCallout: View {
        let day: ForecastDetail.DayWrites
        let date: Date

        var body: some View {
            VStack(alignment: .leading, spacing: 2) {
                Text("\(Format.bytes(day.bytes)) written")
                    .font(.callout.weight(.semibold)).monospacedDigit()
                Text(Calendar.current.isDateInToday(date) ? "Today so far"
                     : date.formatted(.dateTime.weekday(.wide).day().month(.abbreviated)))
                    .font(.caption).foregroundStyle(.secondary)
                ForEach(Array(day.top.prefix(5).enumerated()), id: \.offset) { _, w in
                    HStack {
                        Text(w.name).lineLimit(1)
                        Spacer(minLength: 10)
                        Text(Format.bytes(w.bytes)).monospacedDigit()
                    }
                    .font(.caption)
                }
            }
            .frame(width: 220)
            .padding(.horizontal, 10)
            .padding(.vertical, 6)
            .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 8, style: .continuous))
            .overlay(RoundedRectangle(cornerRadius: 8, style: .continuous).strokeBorder(Theme.hairline))
        }
    }
}

private struct WritersCard: View {
    let rows: [ForecastDetail.Writer]

    var body: some View {
        let shown = rows.prefix(15)
        VStack(alignment: .leading, spacing: 14) {
            SectionHeader(title: "Wrote the most", subtitle: "Apps and processes, the last 24 hours")
            if shown.isEmpty {
                Text("Nothing yet: the watcher counts writes from its next samples.")
                    .font(.callout).foregroundStyle(.secondary)
            }
            ForEach(Array(shown.enumerated()), id: \.offset) { _, w in
                BarRow(name: w.name, value: w.bytes, largest: shown.first?.bytes ?? 1, color: .purple)
            }
        }
        .card()
    }
}
#endif

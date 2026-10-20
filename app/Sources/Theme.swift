// The look, in one place: the palette, the card every section sits on, and
// the pieces the windows are built from (icon tiles, pills, the capacity
// ring, bars, the free-space sparkline), so every window reads as one app.

import AppKit
import Charts
import SwiftUI

enum Theme {
    /// Behind the content: light grey in light mode, near black in dark, as
    /// in System Settings.
    static let canvas = dynamic(light: (0.957, 0.957, 0.965), dark: (0.114, 0.114, 0.122))
    /// The cards on it.
    static let card = dynamic(light: (1, 1, 1), dark: (0.169, 0.169, 0.180))
    static let hairline = Color.primary.opacity(0.07)
    static let radius: CGFloat = 14

    /// The brand: a deep blue into teal, as in the icon.
    static let brand = [Color(red: 0.25, green: 0.42, blue: 1.0), Color(red: 0.18, green: 0.80, blue: 0.86)]

    private static func dynamic(light: (Double, Double, Double), dark: (Double, Double, Double)) -> Color {
        Color(nsColor: NSColor(name: nil) { appearance in
            let c = appearance.bestMatch(from: [.darkAqua, .aqua]) == .darkAqua ? dark : light
            return NSColor(srgbRed: c.0, green: c.1, blue: c.2, alpha: 1)
        })
    }
}

/// How things stand, in one word and one colour.
enum Health {
    /// No forecast yet.
    case learning
    case calm
    /// Full within two weeks, or something runs away.
    case watch
    /// Full within three days, or below the floor.
    case urgent

    init(_ s: Status?) {
        guard let s else { self = .learning; return }
        let soonest = s.incidents.compactMap(\.incident.fullInDays).min() ?? .infinity
        if s.alert >= .within3Days || soonest <= 3 { self = .urgent }
        else if s.alert > .none || !s.incidents.isEmpty { self = .watch }
        else if s.forecast?.rate == nil { self = .learning }
        else { self = .calm }
    }

    var label: String {
        switch self {
        case .learning: "Learning"
        case .calm: "Healthy"
        case .watch: "Needs a look"
        case .urgent: "Act soon"
        }
    }

    var tint: Color {
        switch self {
        case .learning: .blue
        case .calm: .green
        case .watch: .orange
        case .urgent: .red
        }
    }

    var symbol: String {
        switch self {
        case .learning: "hourglass"
        case .calm: "checkmark.circle.fill"
        case .watch: "exclamationmark.circle.fill"
        case .urgent: "exclamationmark.triangle.fill"
        }
    }

    /// The ring's fill, from its start to its end.
    var ring: [Color] {
        switch self {
        case .learning, .calm: Theme.brand
        case .watch: [Color(red: 1.0, green: 0.78, blue: 0.20), Color(red: 1.0, green: 0.50, blue: 0.12)]
        case .urgent: [Color(red: 1.0, green: 0.45, blue: 0.20), Color(red: 0.93, green: 0.16, blue: 0.32)]
        }
    }
}

/// The disk the watcher watches, as Finder names it.
enum Disk {
    static let name: String = (try? URL(fileURLWithPath: "/")
        .resourceValues(forKeys: [.volumeLocalizedNameKey]).volumeLocalizedName) ?? "Macintosh HD"
}

// MARK: - Cards and headers

struct CardBackground: ViewModifier {
    var padding: CGFloat
    var tint: Color?

    func body(content: Content) -> some View {
        let shape = RoundedRectangle(cornerRadius: Theme.radius, style: .continuous)
        content
            .padding(padding)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background {
                shape.fill(Theme.card)
                    .overlay {
                        if let tint {
                            shape.fill(LinearGradient(colors: [tint.opacity(0.13), tint.opacity(0.0)],
                                                      startPoint: .topTrailing, endPoint: .bottomLeading))
                        }
                    }
                    .overlay { shape.strokeBorder(Theme.hairline) }
                    .shadow(color: .black.opacity(0.06), radius: 10, y: 3)
            }
    }
}

extension View {
    func card(padding: CGFloat = 18, tint: Color? = nil) -> some View {
        modifier(CardBackground(padding: padding, tint: tint))
    }
}

/// A page of the main window: a large title, a line under it, and its
/// sections on the canvas.
struct Page<Content: View>: View {
    let title: String
    var subtitle: String?
    @ViewBuilder let content: Content

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 22) {
                VStack(alignment: .leading, spacing: 4) {
                    Text(title).font(.system(size: 28, weight: .bold))
                    if let subtitle {
                        Text(subtitle).font(.title3).foregroundStyle(.secondary)
                    }
                }
                content
            }
            .padding(.horizontal, 32)
            .padding(.top, 8)
            .padding(.bottom, 32)
            .frame(maxWidth: 1080, alignment: .leading)
            .frame(maxWidth: .infinity)
        }
        .background(Theme.canvas)
    }
}

struct SectionHeader<Accessory: View>: View {
    let title: String
    var subtitle: String?
    @ViewBuilder var accessory: Accessory

    var body: some View {
        HStack(alignment: .firstTextBaseline) {
            VStack(alignment: .leading, spacing: 2) {
                Text(title).font(.title3.weight(.semibold))
                if let subtitle {
                    Text(subtitle).font(.callout).foregroundStyle(.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }
            Spacer(minLength: 12)
            accessory
        }
    }
}

extension SectionHeader where Accessory == EmptyView {
    init(title: String, subtitle: String? = nil) {
        self.init(title: title, subtitle: subtitle) { EmptyView() }
    }
}

/// "See all ›": a link to another page.
struct MoreLink: View {
    let title: String
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            HStack(spacing: 3) {
                Text(title)
                Image(systemName: "chevron.right").font(.caption.weight(.semibold))
            }
        }
        .buttonStyle(.plain)
        .foregroundStyle(Color.accentColor)
        .font(.callout.weight(.medium))
    }
}

// MARK: - Small pieces

/// A symbol on a rounded square of colour, as in System Settings.
struct IconTile: View {
    let symbol: String
    let color: Color
    var size: CGFloat = 28

    var body: some View {
        RoundedRectangle(cornerRadius: size * 0.27, style: .continuous)
            .fill(color.gradient)
            .frame(width: size, height: size)
            .overlay {
                Image(systemName: symbol)
                    .font(.system(size: size * 0.5, weight: .semibold))
                    .foregroundStyle(.white)
            }
            .shadow(color: color.opacity(0.25), radius: 2, y: 1)
    }
}

struct Pill: View {
    let text: String
    var color: Color = .secondary
    var symbol: String?

    var body: some View {
        HStack(spacing: 4) {
            if let symbol { Image(systemName: symbol) }
            Text(text)
        }
        .font(.caption.weight(.semibold))
        .monospacedDigit()
        .padding(.horizontal, 8)
        .padding(.vertical, 3)
        .foregroundStyle(color)
        .background(color.opacity(0.14), in: Capsule())
    }
}

struct HealthPill: View {
    let health: Health

    var body: some View {
        Pill(text: health.label, color: health.tint, symbol: health.symbol)
    }
}

/// A label over a value, for a row of facts.
struct Fact: View {
    let label: String
    let value: String

    var body: some View {
        VStack(alignment: .leading, spacing: 3) {
            Text(label.uppercased())
                .font(.caption2.weight(.semibold))
                .tracking(0.6)
                .foregroundStyle(.secondary)
            Text(value)
                .font(.system(.title3, design: .rounded).weight(.semibold))
                .monospacedDigit()
                .lineLimit(1)
                .minimumScaleFactor(0.8)
        }
    }
}

/// How full the disk is: a ring that fills with the brand (or warning)
/// gradient when it first appears, with anything in the middle.
struct CapacityRing<Centre: View>: View {
    let used: Double
    let colors: [Color]
    var lineWidth: CGFloat = 16
    @ViewBuilder var centre: Centre
    @State private var shown: Double = 0

    var body: some View {
        ZStack {
            Circle().stroke(Color.primary.opacity(0.07), lineWidth: lineWidth)
            Circle()
                .trim(from: 0, to: max(0.001, shown))
                .stroke(AngularGradient(colors: colors, center: .center,
                                        startAngle: .degrees(0), endAngle: .degrees(360 * max(shown, 0.05))),
                        style: StrokeStyle(lineWidth: lineWidth, lineCap: .round))
                .rotationEffect(.degrees(-90))
                .shadow(color: (colors.last ?? .blue).opacity(0.35), radius: lineWidth * 0.5)
            centre
        }
        .padding(lineWidth / 2)
        .onAppear {
            withAnimation(.spring(duration: 1.2, bounce: 0.12).delay(0.1)) { shown = used }
        }
        .onChange(of: used) { _, v in
            withAnimation(.spring(duration: 0.8)) { shown = v }
        }
        .accessibilityElement(children: .combine)
        .accessibilityValue("\(Int(used * 100)) percent used")
    }
}

/// A thin capacity bar, for the sidebar and the menu bar panel.
struct CapacityBar: View {
    let used: Double
    let colors: [Color]
    var height: CGFloat = 6

    var body: some View {
        GeometryReader { geo in
            ZStack(alignment: .leading) {
                Capsule().fill(Color.primary.opacity(0.09))
                Capsule()
                    .fill(LinearGradient(colors: colors, startPoint: .leading, endPoint: .trailing))
                    .frame(width: max(height, geo.size.width * min(max(used, 0), 1)))
            }
        }
        .frame(height: height)
    }
}

/// A name, a bar as long as its share of the largest, and a size.
struct BarRow: View {
    let name: String
    var detail: String?
    let value: Int64
    let largest: Int64
    let color: Color
    var help: String?

    var body: some View {
        VStack(alignment: .leading, spacing: 5) {
            HStack(alignment: .firstTextBaseline) {
                Text(name).lineLimit(1).truncationMode(.middle)
                if let detail {
                    Text(detail).foregroundStyle(.secondary).lineLimit(1).truncationMode(.head)
                }
                Spacer(minLength: 8)
                Text(Format.bytes(value)).monospacedDigit().foregroundStyle(.secondary)
            }
            .font(.callout)
            GeometryReader { geo in
                Capsule()
                    .fill(LinearGradient(colors: [color.opacity(0.95), color.opacity(0.55)],
                                         startPoint: .leading, endPoint: .trailing))
                    .frame(width: max(4, geo.size.width * share))
            }
            .frame(height: 5)
        }
        .help(help ?? name)
    }

    private var share: Double {
        largest > 0 ? min(1, Double(value) / Double(largest)) : 0
    }
}

/// Free space as a soft area under a line, no axes: the shape of the last
/// days at a glance.
struct Sparkline: View {
    let series: [[Int64]]
    var color: Color = .accentColor
    var since: TimeInterval = 3 * 86_400

    var body: some View {
        let cutoff = Date().timeIntervalSince1970 - since
        let pts = series.filter { $0.count == 2 && Double($0[0]) >= cutoff }
        let ys = pts.map { Double($0[1]) / 1e9 }
        let lo = ys.min() ?? 0, hi = ys.max() ?? 1
        let pad = max((hi - lo) * 0.15, 0.5)
        Chart {
            ForEach(pts, id: \.self) { p in
                let x = Date(timeIntervalSince1970: TimeInterval(p[0]))
                AreaMark(x: .value("Time", x), yStart: .value("Base", lo - pad), yEnd: .value("Free", Double(p[1]) / 1e9))
                    .foregroundStyle(LinearGradient(colors: [color.opacity(0.30), color.opacity(0.0)],
                                                    startPoint: .top, endPoint: .bottom))
                    .interpolationMethod(.monotone)
                LineMark(x: .value("Time", x), y: .value("Free", Double(p[1]) / 1e9))
                    .foregroundStyle(color)
                    .lineStyle(StrokeStyle(lineWidth: 1.8, lineCap: .round, lineJoin: .round))
                    .interpolationMethod(.monotone)
            }
        }
        .chartXAxis(.hidden)
        .chartYAxis(.hidden)
        .chartLegend(.hidden)
        .chartYScale(domain: (lo - pad)...(hi + pad))
        .accessibilityLabel("Free space over the last days")
    }
}

/// What the headline says about the disk, here and in the menu bar panel.
enum Headline {
    static func title(_ s: Status) -> String {
        if s.alert == .low { return "Your disk is almost full" }
        if let full = s.forecast?.fullIn { return "Full in about \(Format.days(full.days))" }
        // The disk as a whole isn't filling, but something on it is: that
        // leads, or the headline would contradict the card below it.
        if let e = worst(s) {
            let name = subject(e.incident)
            if let d = e.incident.fullInDays { return "\(name) could fill it in \(Format.days(d))" }
            return e.incident.kind == .growth ? "\(name) keeps growing" : "\(name) keeps rewriting the disk"
        }
        guard let f = s.forecast else { return "Waiting for the first sample" }
        guard let r = f.rate else { return "Getting to know your disk" }
        if r.perDay <= 0 { return "Not filling up" }
        return "Filling slowly"
    }

    static func detail(_ s: Status) -> String {
        if s.alert == .low {
            return s.alertSentence ?? "Only \(Format.bytes(s.avail)) left."
        }
        if s.forecast?.fullIn == nil, let e = worst(s) {
            let i = e.incident
            var t = i.kind == .growth
                ? "It grows \(Format.bytes(i.perDay)) a day"
                : "It writes \(Format.bytes(i.perDay)) a day"
            if let r = s.forecast?.rate, r.perDay <= 0 {
                t += ", though free space overall held up over the \(Format.window(r.window)) trend"
            }
            t += "."
            if s.incidents.count > 1 { t += " \(s.incidents.count - 1) more below." }
            return t
        }
        guard let f = s.forecast else { return "The watcher takes a sample every 5 minutes." }
        guard let r = f.rate else {
            return "A forecast needs 8 samples over 3 hours; there are \(s.history.samples) so far."
        }
        if let full = f.fullIn {
            var t = "Around \(Format.date(at: f.at, inDays: full.days)), at \(Format.bytes(r.perDay)) a day."
            if let soon = f.recentFullIn {
                t += " At the last few hours' pace, in \(Format.days(soon.days))."
            }
            return t
        }
        var t: String
        if r.perDay < 0 {
            t = "Free space grew \(Format.bytes(-r.perDay)) a day over the \(Format.window(r.window)) trend."
        } else if r.perDay == 0 {
            t = "Free space held steady over the \(Format.window(r.window)) trend."
        } else {
            t = "At \(Format.bytes(r.perDay)) a day, there's room for a long time."
        }
        return t
    }

    /// The runaway that fills the disk soonest.
    private static func worst(_ s: Status) -> Episode? {
        s.incidents.min { ($0.incident.fullInDays ?? .infinity, -$0.incident.perDay)
            < ($1.incident.fullInDays ?? .infinity, -$1.incident.perDay) }
    }

    /// A folder by its name, a process as it is.
    static func subject(_ i: Incident) -> String {
        i.kind == .growth ? URL(fileURLWithPath: i.subject).lastPathComponent : i.subject
    }

    /// The trend as a fact: ("Filling", "2.1 GB a day"), ("Freeing", …), ("Trend", "Steady").
    static func pace(_ s: Status) -> (label: String, value: String) {
        guard let r = s.forecast?.rate else { return ("Trend", "Learning") }
        if abs(r.perDay) < 50_000_000 { return ("Trend", "Steady") }
        return r.perDay > 0 ? ("Filling", "\(Format.bytes(r.perDay)) a day")
                            : ("Freeing", "\(Format.bytes(-r.perDay)) a day")
    }
}

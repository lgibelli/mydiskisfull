// The look, in one place: the few colours the app uses, what the status
// says, the bar the disk is drawn with, and the small pieces
// rows are built from, so every window reads as one app. The main window's
// pages are native grouped lists (Form) and tables; the cards are for the
// windows of their own (Welcome, About, a profile's review).

import AppKit
import SwiftUI

enum Theme {
    /// Behind the cards: light grey in light mode, near black in dark, as
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
    /// Could be full within two weeks, or something runs away.
    case watch
    /// Full within three days, or below the floor.
    case urgent

    init(_ s: Status?) {
        guard let s else { self = .learning; return }
        let soonest = s.incidents.compactMap(\.incident.fullInDays).min() ?? .infinity
        if s.alert >= .within3Days || soonest <= 3 { self = .urgent }
        else if s.alert > .none || !s.incidents.isEmpty || (s.forecast?.fullIn?.days ?? .infinity) <= 14 {
            self = .watch
        }
        else if s.forecast?.rate == nil { self = .learning }
        else { self = .calm }
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
        case .learning: "hourglass.circle.fill"
        case .calm: "checkmark.circle.fill"
        case .watch: "exclamationmark.circle.fill"
        case .urgent: "exclamationmark.triangle.fill"
        }
    }

    /// The capacity bar: the accent colour until there's something to say.
    var barTint: Color {
        switch self {
        case .learning, .calm: .accentColor
        case .watch: .orange
        case .urgent: .red
        }
    }
}

/// The disk the watcher watches, as Finder names it.
enum Disk {
    static let name: String = (try? URL(fileURLWithPath: "/")
        .resourceValues(forKeys: [.volumeLocalizedNameKey]).volumeLocalizedName) ?? "Macintosh HD"

    /// Its icon, as Finder shows it.
    @MainActor static let icon: NSImage = NSWorkspace.shared.icon(forFile: "/")
}

// MARK: - Cards (the windows of their own)

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

// MARK: - The storage bar

/// The disk as one bar split into coloured parts, a category each, then
/// what's free, labelled like stations on a railway map: a station on each
/// part's edge and a line, in its colour, to its name and size, above and
/// below in turn, moved aside where they'd overlap, the lines bending at 45°
/// and never crossing. A part or its name can be clicked (`onSelect`, with
/// its id): it opens the category.
struct RailBar: View {
    struct Stop {
        /// What a click on it opens; none for what's free.
        let id: String?
        let title: String
        let bytes: Int64
        let color: Color
        /// Free space: its name and line in grey.
        var muted = false
        /// Comes back by itself: striped.
        var hatched = false
    }

    let stops: [Stop]
    let total: Int64
    var onSelect: ((String) -> Void)?

    private static let barHeight: CGFloat = 22
    private static let labelHeight: CGFloat = 30
    private static let band: CGFloat = 36
    private static let gap: CGFloat = 1.5
    private static var top: CGFloat { labelHeight + band }
    private static var bottom: CGFloat { top + barHeight }

    var body: some View {
        GeometryReader { geo in
            let l = Self.layout(stops, total: total, width: geo.size.width)
            ZStack(alignment: .topLeading) {
                Canvas { ctx, _ in draw(ctx, l) }
                ForEach(Array(stops.indices), id: \.self) { i in
                    let span = l.spans[i]
                    Hit(stop: stops[i], select: select) { Color.clear }
                        .frame(width: max(2, span.1 - span.0), height: Self.barHeight)
                        .position(x: (span.0 + span.1) / 2, y: Self.top + Self.barHeight / 2)
                }
                ForEach(l.labels, id: \.stop) { p in
                    let s = stops[p.stop]
                    Hit(stop: s, select: select) {
                        VStack(spacing: 0) {
                            Text(s.title).font(.caption.weight(.semibold))
                                .foregroundStyle(s.muted ? Color.secondary : Color.primary)
                            Text(Format.bytes(s.bytes)).font(.caption2).monospacedDigit()
                                .foregroundStyle(.secondary)
                        }
                        .fixedSize()
                    }
                    .position(x: p.x, y: p.up ? Self.labelHeight + 2 - 15 : Self.bottom + Self.band - 2 + 15)
                }
            }
        }
        .frame(height: Self.labelHeight * 2 + Self.band * 2 + Self.barHeight)
        .accessibilityElement(children: .contain)
    }

    private func select(_ s: Stop) {
        if let id = s.id { onSelect?(id) }
    }

    /// A part of the bar or its name: the pointing hand over it, and a
    /// click selects it.
    private struct Hit<Label: View>: View {
        let stop: Stop
        let select: (Stop) -> Void
        @ViewBuilder var label: Label
        @State private var hovering = false

        var body: some View {
            label
                .contentShape(Rectangle())
                .opacity(hovering ? 0.7 : 1)
                .onHover { on in
                    guard stop.id != nil else { return }
                    hovering = on
                    if on { NSCursor.pointingHand.push() } else { NSCursor.pop() }
                }
                .onTapGesture { select(stop) }
                .help(stop.id == nil ? "\(stop.title): \(Format.bytes(stop.bytes))"
                      : "\(stop.title): \(Format.bytes(stop.bytes)). Click to see what's in it.")
                .accessibilityAddTraits(stop.id == nil ? [] : .isButton)
                .accessibilityLabel("\(stop.title), \(Format.bytes(stop.bytes))")
        }
    }

    // MARK: - Layout

    private struct Placed {
        let stop: Int
        let up: Bool
        /// The station, on the part's middle.
        let center: CGFloat
        let width: CGFloat
        /// The label's middle.
        var x: CGFloat
        /// How far from the bar its line turns.
        var level = 0
    }

    private struct Layout {
        var spans: [(CGFloat, CGFloat)] = []
        var labels: [Placed] = []
    }

    /// The parts, as wide as their share (the last one, what's free, takes
    /// what's left), at least 2 points each; then the names, above and below
    /// in turn, each side spread on its own.
    private static func layout(_ stops: [Stop], total: Int64, width: CGFloat) -> Layout {
        var l = Layout()
        let room = width - gap * CGFloat(max(stops.count - 1, 0))
        var x: CGFloat = 0
        for (i, s) in stops.enumerated() {
            let w = i == stops.count - 1 ? max(2, width - x) : max(2, room * CGFloat(s.bytes) / CGFloat(max(total, 1)))
            l.spans.append((x, min(width, x + w)))
            x += w + gap
        }
        for side in [0, 1] {
            var labels: [Placed] = stops.indices.filter { $0 % 2 == side }.map { i in
                let c = (l.spans[i].0 + l.spans[i].1) / 2
                return Placed(stop: i, up: side == 0, center: c, width: textWidth(stops[i]), x: c)
            }
            spread(&labels, width: width)
            // A label moved a point or two goes straight over its station.
            for k in labels.indices where abs(labels[k].x - labels[k].center) < 3 {
                labels[k].x = labels[k].center
            }
            l.labels += labels
        }
        return l
    }

    /// As wide as its name or its size, whichever is wider.
    private static func textWidth(_ s: Stop) -> CGFloat {
        let caption = NSFont.preferredFont(forTextStyle: .caption1).pointSize
        let caption2 = NSFont.preferredFont(forTextStyle: .caption2).pointSize
        let title = (s.title as NSString).size(withAttributes: [.font: NSFont.systemFont(ofSize: caption, weight: .semibold)])
        let size = (Format.bytes(s.bytes) as NSString)
            .size(withAttributes: [.font: NSFont.monospacedDigitSystemFont(ofSize: caption2, weight: .regular)])
        return ceil(max(title.width, size.width)) + 2
    }

    /// Moves labels apart so none overlaps the next, and keeps them inside
    /// the width; then gives each moved one the height its line turns at,
    /// so lines going the same way never cross.
    private static func spread(_ labels: inout [Placed], width: CGFloat) {
        let space: CGFloat = 10
        labels.sort { $0.center < $1.center }
        for i in labels.indices {
            var x = max(labels[i].x, labels[i].width / 2)
            if i > 0 { x = max(x, labels[i - 1].x + (labels[i - 1].width + labels[i].width) / 2 + space) }
            labels[i].x = x
        }
        for i in labels.indices.reversed() {
            var x = min(labels[i].x, width - labels[i].width / 2)
            if i < labels.count - 1 {
                x = min(x, labels[i + 1].x - (labels[i].width + labels[i + 1].width) / 2 - space)
            }
            labels[i].x = x
        }
        // Moved right: the rightmost of a run turns nearest the bar; moved
        // left, the leftmost.
        var i = 0
        while i < labels.count {
            let dir = labels[i].x - labels[i].center
            guard abs(dir) >= 1 else { i += 1; continue }
            var j = i
            while j + 1 < labels.count, (labels[j + 1].x - labels[j + 1].center) * dir > 0,
                  abs(labels[j + 1].x - labels[j + 1].center) >= 1 { j += 1 }
            for k in i...j { labels[k].level = (dir > 0 ? j - k : k - i) % 3 }
            i = j + 1
        }
    }

    // MARK: - Drawing

    private func draw(_ ctx: GraphicsContext, _ l: Layout) {
        let width = l.spans.last?.1 ?? 0
        var bar = ctx
        bar.clip(to: Path(roundedRect: CGRect(x: 0, y: Self.top, width: width, height: Self.barHeight), cornerRadius: 6))
        for (i, s) in stops.enumerated() {
            let r = CGRect(x: l.spans[i].0, y: Self.top, width: max(0, l.spans[i].1 - l.spans[i].0), height: Self.barHeight)
            bar.fill(Path(r), with: .linearGradient(Gradient(colors: [s.color.opacity(0.85), s.color]),
                                                    startPoint: CGPoint(x: 0, y: Self.top),
                                                    endPoint: CGPoint(x: 0, y: Self.bottom)))
            if s.hatched {
                var h = bar
                h.clip(to: Path(r))
                var lines = Path()
                var x = r.minX - Self.barHeight
                while x < r.maxX {
                    lines.move(to: CGPoint(x: x, y: Self.bottom))
                    lines.addLine(to: CGPoint(x: x + Self.barHeight, y: Self.top))
                    x += 7
                }
                h.stroke(lines, with: .color(Theme.card.opacity(0.55)), lineWidth: 2.5)
            }
        }
        for p in l.labels {
            let s = stops[p.stop]
            let edge = p.up ? Self.top : Self.bottom
            let end = p.up ? Self.labelHeight + 2 : Self.bottom + Self.band - 2
            let line = s.muted ? Color.secondary.opacity(0.6) : s.color
            ctx.stroke(Self.leader(from: p.center, to: p.x, edge: edge, end: end, level: p.level, up: p.up),
                       with: .color(line), style: StrokeStyle(lineWidth: 2, lineCap: .round, lineJoin: .round))
            // The station, on the bar's edge.
            let dot = CGRect(x: p.center - 4, y: edge - 4, width: 8, height: 8)
            ctx.fill(Path(ellipseIn: dot), with: .color(Theme.card))
            ctx.stroke(Path(ellipseIn: dot), with: .color(line), lineWidth: 2)
        }
    }

    /// From the station at `from` on the bar's `edge` to the label at `to`
    /// (its near edge at `end`): straight, or up (down), across at its
    /// level's height and up (down) again, the corners cut at 45°.
    private static func leader(from: CGFloat, to: CGFloat, edge: CGFloat, end: CGFloat, level: Int, up: Bool) -> Path {
        let s: CGFloat = up ? -1 : 1
        var p = Path()
        p.move(to: CGPoint(x: from, y: edge))
        let dx = to - from
        guard abs(dx) >= 1 else {
            p.addLine(to: CGPoint(x: from, y: end))
            return p
        }
        let c = min(6, abs(dx) / 2)
        let h: CGFloat = dx > 0 ? 1 : -1
        let run = edge + s * (8 + CGFloat(level) * 6 + c)
        p.addLine(to: CGPoint(x: from, y: run - s * c))
        p.addLine(to: CGPoint(x: from + h * c, y: run))
        p.addLine(to: CGPoint(x: to - h * c, y: run))
        p.addLine(to: CGPoint(x: to, y: run + s * c))
        p.addLine(to: CGPoint(x: to, y: end))
        return p
    }
}

extension RailBar.Stop {
    /// A report's categories, biggest first (past `limit`, the rest as
    /// one); then what comes back by itself (purgeable, swap, a macOS update
    /// waiting for a restart), taken out of the category that counts it and
    /// drawn hatched beside what's free; then what's free.
    static func of(_ r: SpaceReport, limit: Int = .max) -> [RailBar.Stop] {
        let kinds = r.kinds.filter { $0.bytes > 0 }.sorted { $0.bytes > $1.bytes }
        var left = Dictionary(kinds.map { ($0.id, $0.bytes) }, uniquingKeysWith: +)
        let back: [RailBar.Stop] = r.byItself.compactMap { b in
            guard b.bytes >= 500_000_000, let k = b.kind, let has = left[k], has > 0 else { return nil }
            let bytes = min(b.bytes, has)
            left[k] = has - bytes
            return RailBar.Stop(id: k, title: SpaceText.label(b.what), bytes: bytes, color: .green, hatched: true)
        }
        let shown = kinds.prefix(limit).compactMap { k in
            let bytes = left[k.id] ?? 0
            return bytes > 0 ? RailBar.Stop(id: k.id, title: k.title, bytes: bytes, color: SpaceText.color(k.id)) : nil
        }
        let rest = kinds.dropFirst(limit).reduce(0) { $0 + (left[$1.id] ?? 0) }
        let others = rest > 0 ? [RailBar.Stop(id: nil, title: "Everything else", bytes: rest, color: Color(white: 0.7))] : []
        return shown + others + back
            + [RailBar.Stop(id: nil, title: "Free", bytes: r.volume.free, color: Color.secondary.opacity(0.18), muted: true)]
    }
}

/// Views in a row, wrapping onto the next line when there's no more room.
struct FlowLayout: Layout {
    var spacing: CGFloat = 12
    var lineSpacing: CGFloat = 6

    func sizeThatFits(proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) -> CGSize {
        let width = proposal.width ?? .infinity
        var x: CGFloat = 0, y: CGFloat = 0, line: CGFloat = 0, widest: CGFloat = 0
        for s in subviews {
            let size = s.sizeThatFits(.unspecified)
            if x > 0, x + size.width > width {
                y += line + lineSpacing
                x = 0
                line = 0
            }
            x += size.width + spacing
            widest = max(widest, x - spacing)
            line = max(line, size.height)
        }
        return CGSize(width: proposal.width ?? widest, height: y + line)
    }

    func placeSubviews(in bounds: CGRect, proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) {
        var x = bounds.minX, y = bounds.minY, line: CGFloat = 0
        for s in subviews {
            let size = s.sizeThatFits(.unspecified)
            if x > bounds.minX, x + size.width > bounds.maxX {
                y += line + lineSpacing
                x = bounds.minX
                line = 0
            }
            s.place(at: CGPoint(x: x, y: y), proposal: .unspecified)
            x += size.width + spacing
            line = max(line, size.height)
        }
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

/// What can be done with a file or folder besides its button, in a ⋯ menu
/// at the end of its row: show it, copy its path, keep it.
struct PathMenu: View {
    let path: String
    /// Never suggest deleting it again; nil where it can't be kept.
    var keep: (() -> Void)?

    var body: some View {
        Menu {
            PathMenuItems(path: path, keep: keep)
        } label: {
            Image(systemName: "ellipsis.circle")
        }
        .menuStyle(.borderlessButton)
        .menuIndicator(.hidden)
        .fixedSize()
        .foregroundStyle(.secondary)
        .help("More")
    }
}

/// The same, for a context menu.
struct PathMenuItems: View {
    let path: String
    var keep: (() -> Void)?

    var body: some View {
        Button("Show in Finder") {
            NSWorkspace.shared.activateFileViewerSelecting([URL(fileURLWithPath: path)])
        }
        // Whole and as it is (no `~`, no quotes), for Terminal or Go to Folder.
        Button("Copy Path") {
            NSPasteboard.general.clearContents()
            NSPasteboard.general.setString(path, forType: .string)
        }
        if let keep {
            Divider()
            Button("Keep It") { keep() }
                .help("Never suggest deleting, cleaning or moving it again. Settings lists what you keep.")
        }
    }
}

/// A thin capacity bar, for the sidebar.
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
    var sign = ""
    var help: String?

    var body: some View {
        VStack(alignment: .leading, spacing: 5) {
            HStack(alignment: .firstTextBaseline) {
                Text(name).lineLimit(1).truncationMode(.middle)
                if let detail {
                    Text(detail).foregroundStyle(.secondary).lineLimit(1).truncationMode(.head)
                }
                Spacer(minLength: 8)
                Text(sign + Format.bytes(value)).monospacedDigit().foregroundStyle(.secondary)
            }
            GeometryReader { geo in
                Capsule()
                    .fill(color.opacity(0.8))
                    .frame(width: max(4, geo.size.width * share))
            }
            .frame(height: 4)
        }
        .help(help ?? name)
    }

    private var share: Double {
        largest > 0 ? min(1, Double(value) / Double(largest)) : 0
    }
}

/// What the status says about the disk, on Overview, History and in the
/// menu bar.
enum Headline {
    static func title(_ s: Status) -> String {
        if s.alert == .low { return "Your disk is almost full" }
        if let f = s.forecast, let full = f.fullIn {
            // Filling on average: when. Otherwise some days fill it and
            // others free it, and it only could be.
            return (f.rate?.perDay ?? 0) > 0
                ? "Full in about \(Format.days(full.days))"
                : "Could be full in about \(Format.days(full.days))"
        }
        // The disk as a whole isn't filling, but something on it is: that
        // leads, or the status would contradict what's under it.
        if let e = worst(s) {
            if let d = e.incident.fullInDays { return "\(subject(e)) could fill it in \(Format.days(d))" }
            return e.title
        }
        guard let f = s.forecast else { return "Waiting for the first sample" }
        guard let r = f.rate else { return "Getting to know your disk" }
        if r.perDay < 50_000_000 { return "Not filling up" }
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
                t += ", though free space overall held up"
            }
            return t + "."
        }
        guard let f = s.forecast else { return "The watcher takes a sample every 5 minutes." }
        guard let r = f.rate else {
            return "A forecast needs 8 samples over 3 hours; there are \(s.history.samples) so far."
        }
        if let full = f.fullIn {
            let date = Format.date(at: f.at, inDays: full.days)
            var t: String
            if r.perDay > 0 {
                t = "Around \(date), at \(Format.bytes(r.perDay)) a day"
                if let p = f.purgeable, p >= 1_000_000_000 {
                    t += ", counting the \(Format.bytes(p)) macOS frees by itself"
                }
                t += "."
            } else {
                t = "Some days fill it and others free it. If the next days are like the last ones, it could be full by \(date)."
            }
            // How sure: the nearest horizon with a real chance.
            if let p = f.fullBy {
                if p.d1 >= 0.1 {
                    t += " \(Format.percent(p.d1)) likely within a day."
                } else if p.d3 >= 0.1 {
                    t += " \(Format.percent(p.d3)) likely within 3 days."
                } else if r.perDay <= 0 {
                    t += " \(Format.percent(p.d14)) likely within 2 weeks."
                }
            } else if let soon = f.recentFullIn {
                t += " At the last few hours' pace, in \(Format.days(soon.days))."
            }
            if let a = f.arrivals?.max(by: { $0.bytes < $1.bytes }) {
                t += " One-offs, like the \(Format.bytes(a.bytes)) that arrived on \(Format.day(a.ts)), don't count as a pace."
            }
            return t
        }
        // Free space that grew is clean-ups, not a pace to project.
        if r.perDay < 0 { return "Free space hasn't been shrinking lately." }
        if r.perDay < 50_000_000 { return "Free space held steady over the \(Format.window(r.window)) trend." }
        return "At \(Format.bytes(r.perDay)) a day, there's room for a long time."
    }

    /// The runaway that fills the disk soonest.
    private static func worst(_ s: Status) -> Episode? {
        s.incidents.min { ($0.incident.fullInDays ?? .infinity, -$0.incident.perDay)
            < ($1.incident.fullInDays ?? .infinity, -$1.incident.perDay) }
    }

    /// A folder by its name, a process as it is.
    static func subject(_ e: Episode) -> String {
        let i = e.incident
        return i.kind == .growth ? URL(fileURLWithPath: i.subject).lastPathComponent : e.app ?? i.subject
    }
}

// Where the space goes: one bar for the whole disk, split by kind (apps,
// build output, caches, macOS…); a click on a kind opens it under the bar,
// its biggest items with what can be done, and what an app says about one
// (Signal's attachments, Spotlight's index) with it. What may not be needed
// is marked on each: an app not opened in months, files unchanged for a
// year, build output, caches. A suggestion about an item shows with it (its
// growth, its button); the rest follow under More Fixes. What looks
// forgotten is on Cruft. From `mdif space`, a whole-disk scan it caches.

import AppKit
import SwiftUI

struct SpacePage: View {
    @ObservedObject var monitor: Monitor

    var body: some View {
        Page(title: "Space", subtitle: "Where your space goes, and what of it you may not need.") {
            SpaceSection(monitor: monitor)
            MoreFixes(monitor: monitor)
        }
    }
}

struct SpaceSection: View {
    @ObservedObject var monitor: Monitor
    /// The kind chosen on the bar, open under it.
    @State private var selected: String?

    init(monitor: Monitor, selected: String? = nil) {
        self.monitor = monitor
        _selected = State(initialValue: selected)
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            SectionHeader(title: "The whole disk", subtitle: subtitle) {
                if monitor.spaceMeasuring {
                    ProgressView().controlSize(.small)
                } else if monitor.space != nil {
                    Button {
                        Task { await monitor.loadSpace(refresh: true) }
                    } label: {
                        Label("Measure Again", systemImage: "arrow.clockwise")
                    }
                    .buttonStyle(.borderless)
                    .controlSize(.small)
                }
            }
            if !monitor.spaceReadable {
                ReadableNote(monitor: monitor)
            } else if let r = monitor.space {
                if let s = monitor.status, !monitor.spaceMeasuring {
                    ChangedSince(freed: s.avail - r.volume.free, monitor: monitor)
                }
                VStack(alignment: .leading, spacing: 14) {
                    SpaceBar(report: r, selected: selected) { id in
                        withAnimation(.snappy) { selected = selected == id ? nil : id }
                    }
                    if let id = selected, let k = r.kinds.first(where: { $0.id == id }) {
                        Divider()
                        // What the apps keep isn't in them: it's under App data.
                        let data = k.id == "apps" ? r.kinds.first { $0.id == "appdata" } : nil
                        KindInline(kind: k, see: data, monitor: monitor) { id in
                            withAnimation(.snappy) { selected = id }
                        }
                        .transition(.opacity.combined(with: .move(edge: .top)))
                    } else {
                        Label("Click a part of the bar to see what's in it, what you may not need, and what you can do about it.",
                              systemImage: "hand.point.up.left.fill")
                            .font(.callout)
                            .foregroundStyle(.secondary)
                    }
                }
                .card(padding: 16)
            } else if let e = monitor.spaceError, !monitor.spaceMeasuring {
                Label(e, systemImage: "exclamationmark.triangle.fill")
                    .foregroundStyle(.orange)
                    .card()
            } else {
                HStack(spacing: 10) {
                    ProgressView().controlSize(.small)
                    Text("Measuring the whole disk: about a minute the first time. It reads names, sizes and dates, never what's in a file.")
                        .font(.callout).foregroundStyle(.secondary)
                }
                .card()
            }
        }
        .task { await monitor.loadSpace() }
    }

    private var subtitle: String {
        guard let r = monitor.space else { return "Biggest first, with what you may not need marked." }
        let waste = r.kinds.reduce(0) { $0 + $1.reclaimable }
        let measured = "Measured \(Format.ago(r.measured))"
        return waste >= 1_000_000_000
            ? "\(Format.bytes(waste)) of it you may not need. \(measured)."
            : "Biggest first. \(measured)."
    }
}

/// The disk changed since it was measured (something deleted, a download):
/// what's free now is live, the breakdown isn't until it's measured again.
private struct ChangedSince: View {
    let freed: Int64
    @ObservedObject var monitor: Monitor

    var body: some View {
        if abs(freed) >= 1_000_000_000 {
            HStack(spacing: 10) {
                Image(systemName: freed > 0 ? "arrow.down.circle.fill" : "arrow.up.circle.fill")
                    .foregroundStyle(freed > 0 ? .green : .orange)
                Text(freed > 0
                     ? "\(Format.bytes(freed)) more free since this was measured."
                     : "\(Format.bytes(-freed)) less free since this was measured.")
                    .font(.callout)
                Spacer()
                Button("Measure Again") { Task { await monitor.loadSpace(refresh: true) } }
                    .controlSize(.small)
            }
            .card(padding: 12, tint: freed > 0 ? .green : .orange)
        }
    }
}

/// In the App Sandbox with one folder chosen: the whole disk is needed.
private struct ReadableNote: View {
    @ObservedObject var monitor: Monitor

    var body: some View {
        HStack(spacing: 12) {
            Image(systemName: "internaldrive").font(.title2).foregroundStyle(.secondary)
            Text("To see where all of the space goes, let \(Edition.name) see the whole disk, not one folder.")
                .font(.callout).foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
            Spacer(minLength: 8)
            #if APP_STORE
            Button("Choose the Disk…") { monitor.chooseDisk() }
            #endif
        }
        .card()
    }
}

/// The disk as one bar: each kind in its colour, then what's free, each
/// labelled like a station on a railway map (RailBar).
private struct SpaceBar: View {
    let report: SpaceReport
    /// The kind open under the bar: the others dim.
    var selected: String?
    /// A kind clicked on the bar.
    var onSelect: ((String) -> Void)?

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            RailBar(stops: stops, total: max(report.volume.total, 1), selected: selected, onSelect: onSelect)
            HStack(spacing: 14) {
                Text("\(Format.bytes(report.volume.used)) used")
                    .font(.callout.weight(.semibold)).monospacedDigit()
                Text("\(Format.bytes(report.volume.free)) free")
                    .font(.callout).foregroundStyle(.secondary).monospacedDigit()
                // Counted as used, freed without a clean-up.
                ForEach(report.byItself.filter { $0.bytes >= 500_000_000 }, id: \.what) { p in
                    Text("\(Format.bytes(p.bytes)) \(SpaceText.phrase(p.what))")
                        .font(.callout).foregroundStyle(.green).monospacedDigit()
                        .help("\(SpaceText.label(p.what)): \(p.how).")
                }
                if report.trash >= 100_000_000 {
                    Text("\(Format.bytes(report.trash)) in the Trash")
                        .font(.callout).foregroundStyle(.orange).monospacedDigit()
                }
                Spacer()
            }
        }
    }

    /// Each kind, then what comes back by itself (purgeable, swap, a macOS
    /// update waiting for a restart) taken out of the kind that counts it
    /// and drawn hatched beside what's free, then what's free.
    private var stops: [RailBar.Stop] {
        let kinds = report.kinds.filter { $0.bytes > 0 }
        var left = Dictionary(kinds.map { ($0.id, $0.bytes) }, uniquingKeysWith: +)
        let back: [RailBar.Stop] = report.byItself.compactMap { b in
            guard b.bytes >= 500_000_000, let k = b.kind, let has = left[k], has > 0 else { return nil }
            let bytes = min(b.bytes, has)
            left[k] = has - bytes
            return RailBar.Stop(id: k, title: SpaceText.label(b.what), bytes: bytes,
                                color: .green, hatched: true)
        }
        return kinds.compactMap { k in
            let bytes = left[k.id] ?? 0
            return bytes > 0 ? RailBar.Stop(id: k.id, title: k.title, bytes: bytes, color: SpaceText.color(k.id)) : nil
        } + back + [RailBar.Stop(id: nil, title: "Free", bytes: report.volume.free,
                                 color: Color.secondary.opacity(0.18), muted: true)]
    }
}

/// A bar split into coloured parts, each with a station on its edge and a
/// line, in its colour, to its name and size: above and below in turn,
/// moved aside where they'd overlap, the lines bending at 45° and never
/// crossing. A part or its name can be clicked (`onSelect`, with its id).
struct RailBar: View {
    struct Stop {
        /// What a click on it selects; none for what's free.
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
    /// The stop chosen: the others dim.
    var selected: String?
    var onSelect: ((String) -> Void)?

    /// How bright a stop is: dimmed while another is chosen.
    private func shade(_ s: Stop) -> Double {
        selected == nil || s.id == selected ? 1 : 0.3
    }

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
                    .opacity(shade(s))
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
                      : "\(stop.title): \(Format.bytes(stop.bytes)). Click to see what's in it, again to close it.")
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
            bar.opacity = shade(s)
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
            let line = (s.muted ? Color.secondary.opacity(0.6) : s.color).opacity(shade(s))
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

/// A few items from every kind worth a look first: what apps keep, what
/// looks forgotten (on Cruft). Each says what it is.
struct HighlightCard: View {
    let title: String
    let subtitle: String
    let symbol: String
    let tint: Color
    let items: [SpaceReport.Item]
    /// What of each counts here.
    let size: (SpaceReport.Item) -> Int64
    @ObservedObject var monitor: Monitor

    var body: some View {
        let total = items.reduce(0) { $0 + size($1) }
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 12) {
                IconTile(symbol: symbol, color: tint, size: 30)
                VStack(alignment: .leading, spacing: 2) {
                    Text(title).font(.headline)
                    Text(subtitle).font(.caption).foregroundStyle(.secondary)
                }
                Spacer(minLength: 8)
                Text(Format.bytes(total))
                    .font(.system(.title3, design: .rounded).weight(.semibold))
                    .monospacedDigit()
            }
            VStack(spacing: 0) {
                ForEach(Array(items.enumerated()), id: \.element.id) { i, item in
                    if i > 0 { Divider().padding(.leading, 18) }
                    ItemRow(item: item, suggestion: monitor.suggestion(about: item), monitor: monitor)
                }
            }
            .padding(.top, 10)
            .padding(.leading, 18)
        }
        .card(padding: 14, tint: tint)
    }
}

/// What an app keeps in an item, from its handler in `mdif`: what it is,
/// how to make it smaller, and a button to the place (the app, or System
/// Settings).
private struct AboutBox: View {
    let about: SpaceReport.About
    @ObservedObject var monitor: Monitor

    var body: some View {
        let target = Self.target(about.open)
        HStack(alignment: .top, spacing: 10) {
            if let app = about.open?.app,
               let url = NSWorkspace.shared.urlForApplication(withBundleIdentifier: app) {
                Image(nsImage: NSWorkspace.shared.icon(forFile: url.path))
                    .resizable().frame(width: 28, height: 28)
            } else {
                Image(systemName: "lightbulb.fill")
                    .font(.title3).foregroundStyle(.yellow)
                    .frame(width: 28, height: 28)
            }
            VStack(alignment: .leading, spacing: 4) {
                HStack(alignment: .firstTextBaseline) {
                    Text(about.title).font(.callout.weight(.semibold))
                    Text(Format.bytes(about.bytes))
                        .font(.callout.weight(.semibold)).monospacedDigit()
                        .foregroundStyle(.secondary)
                }
                Text(Format.inline(about.what))
                    .font(.caption).foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
                Text(Format.inline(about.how))
                    .font(.callout)
                    .fixedSize(horizontal: false, vertical: true)
                #if APP_STORE
                if let action = about.action {
                    LiteCommand(action: action, monitor: monitor)
                }
                #endif
                HStack(spacing: 10) {
                    #if !APP_STORE
                    if let action = about.action {
                        CleanButton(action: action, command: nil, monitor: monitor)
                            .controlSize(.small)
                            .fixedSize()
                    }
                    #endif
                    if let open = about.open, let target {
                        Button(open.label) { NSWorkspace.shared.open(target) }
                            .controlSize(.small)
                    }
                }
                .padding(.top, 2)
            }
            Spacer(minLength: 0)
        }
        .padding(10)
        .background(Color.secondary.opacity(0.07), in: RoundedRectangle(cornerRadius: 10, style: .continuous))
    }

    /// Where the button goes: the app, if it's on this Mac, or the URL.
    static func target(_ open: SpaceReport.About.Open?) -> URL? {
        guard let open else { return nil }
        if let app = open.app { return NSWorkspace.shared.urlForApplication(withBundleIdentifier: app) }
        return open.url.flatMap(URL.init(string:))
    }
}

/// A kind chosen on the bar, open under it: its name, its size, what of it
/// you may not need, and its items, with ✕ to close it.
private struct KindInline: View {
    let kind: SpaceReport.Kind
    /// Another kind to point to (Apps: App data, where what they keep is).
    let see: SpaceReport.Kind?
    @ObservedObject var monitor: Monitor
    /// Opens another kind, or closes this one with nil.
    let select: (String?) -> Void

    var body: some View {
        let k = kind
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 12) {
                RoundedRectangle(cornerRadius: 3)
                    .fill(SpaceText.color(k.id).gradient)
                    .frame(width: 6, height: 30)
                Text(k.title).font(.headline)
                Spacer(minLength: 8)
                if k.reclaimable >= 500_000_000 {
                    Pill(text: "\(Format.bytes(k.reclaimable)) you may not need", color: .orange)
                }
                Text(Format.bytes(k.bytes))
                    .font(.system(.title3, design: .rounded).weight(.semibold))
                    .monospacedDigit()
                Button { select(nil) } label: {
                    Image(systemName: "xmark.circle.fill").font(.title3)
                }
                .buttonStyle(.borderless)
                .foregroundStyle(.secondary)
                .help("Close")
            }
            if let d = see {
                Button { select(d.id) } label: {
                    Label("What they keep (data, attachments, caches) is under \(d.title): \(Format.bytes(d.bytes))",
                          systemImage: "arrow.right.circle.fill")
                }
                .buttonStyle(.link)
                .font(.callout)
                .padding(.top, 10)
                .padding(.leading, 18)
            }
            KindCaches(kind: k, monitor: monitor)
            KindList(kind: k, monitor: monitor)
        }
        .id(k.id)
    }
}

/// A kind's caches at once, without opening it: which ones is chosen in a
/// sheet.
private struct KindCaches: View {
    let kind: SpaceReport.Kind
    @ObservedObject var monitor: Monitor
    @State private var picking = false

    var body: some View {
        #if !APP_STORE
        let caches = SpaceText.emptiable(kind)
        if caches.count >= 2 {
            let busy = caches.contains { monitor.cleaning.contains($0.path) }
            HStack(spacing: 10) {
                Button {
                    picking = true
                } label: {
                    Label("Empty Caches…", systemImage: "trash")
                }
                .buttonStyle(.borderedProminent)
                .disabled(busy)
                if busy {
                    ProgressView().controlSize(.small)
                    Text("Emptying…").font(.callout).foregroundStyle(.secondary)
                }
            }
            .padding(.top, 10)
            .padding(.leading, 18)
            .sheet(isPresented: $picking) {
                CachePicker(items: caches, monitor: monitor)
            }
        }
        #endif
    }
}

/// A kind's biggest items, the first few or all, and what isn't listed, so
/// the rows add up to the kind.
private struct KindList: View {
    let kind: SpaceReport.Kind
    @ObservedObject var monitor: Monitor
    /// All of its items, not only the first few.
    @State private var all = false
    private static let first = 8

    var body: some View {
        let k = kind
        // The first few, and any an app has something to say about.
        let shown = all ? k.items : k.items.enumerated()
            .filter { $0.offset < Self.first || SpaceText.advised($0.element) }.map(\.element)
        let hidden = k.items.count - shown.count
        // What isn't listed, so the rows add up to the kind.
        let rest = k.bytes - shown.reduce(0) { $0 + $1.bytes }
        VStack(spacing: 0) {
            ForEach(Array(shown.enumerated()), id: \.element.id) { i, item in
                if i > 0 { Divider().padding(.leading, 18) }
                ItemRow(item: item, suggestion: monitor.suggestion(about: item), monitor: monitor)
            }
            if all || hidden > 0 {
                Button(all ? "Show Fewer" : "Show \(hidden) More") {
                    withAnimation(.snappy) { all.toggle() }
                }
                .buttonStyle(.link)
                .font(.callout)
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.vertical, 8)
            }
            if rest >= 100_000_000 {
                Divider().padding(.leading, 18)
                HStack {
                    VStack(alignment: .leading, spacing: 2) {
                        Text("Everything else").font(.callout.weight(.medium))
                        Text(hidden == 0
                             ? "In folders too small to list on their own."
                             : "In the folders not shown above, and ones too small to list.")
                            .font(.caption).foregroundStyle(.secondary)
                    }
                    Spacer()
                    Text(Format.bytes(rest))
                        .font(.system(.callout, design: .rounded).weight(.semibold))
                        .monospacedDigit()
                        .foregroundStyle(.secondary)
                        .padding(.trailing, 26)
                }
                .padding(.vertical, 10)
            }
            if k.items.isEmpty {
                Text("Nothing big enough to list on its own.")
                    .font(.callout).foregroundStyle(.secondary)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(.vertical, 8)
            }
        }
        .padding(.top, 10)
        .padding(.leading, 18)
        .padding(.top, 10)
        .padding(.leading, 18)
    }
}

/// One item: what it is, what marks it, its size, and what can be done.
struct ItemRow: View {
    let item: SpaceReport.Item
    /// A suggestion about it, shown with it.
    let suggestion: Suggestion?
    @ObservedObject var monitor: Monitor

    var body: some View {
        let i = item
        let keep = !i.has("kept") && SpaceText.mayGo(i)
        let fix = suggestion != nil || i.action != nil
        VStack(alignment: .leading, spacing: 8) {
            HStack(alignment: .firstTextBaseline, spacing: 10) {
                VStack(alignment: .leading, spacing: 2) {
                    // Named by its path already: once is enough, and the
                    // copy button goes by the name.
                    let named = Format.path(i.path) == i.name
                    HStack(spacing: 4) {
                        Text(i.name).font(.callout.weight(.medium)).lineLimit(1)
                        if named, !i.path.isEmpty { CopyPathButton(path: i.path) }
                    }
                    if !named, !i.path.isEmpty {
                        HStack(spacing: 4) {
                            Text(Format.path(i.path)).font(.caption).foregroundStyle(.secondary)
                                .lineLimit(1).truncationMode(.middle)
                            CopyPathButton(path: i.path)
                        }
                    }
                }
                Spacer(minLength: 8)
                ForEach(SpaceText.marks(i), id: \.0) { m in
                    Pill(text: m.0, color: m.1, symbol: m.2)
                }
                VStack(alignment: .trailing, spacing: 1) {
                    Text(Format.bytes(i.bytes))
                        .font(.system(.callout, design: .rounded).weight(.semibold))
                        .monospacedDigit()
                    // Nothing to free (macOS's own, a cloud copy's clones) says nothing.
                    if i.frees > 0, i.frees + 500_000_000 < i.bytes {
                        Text("frees \(Format.bytes(i.frees))")
                            .font(.caption2).foregroundStyle(.secondary).monospacedDigit()
                            .help("Part of it is shared with other files (clones or hard links): deleting it frees only this much.")
                    }
                }
                .frame(minWidth: 72, alignment: .trailing)
                if !i.path.isEmpty {
                    Button {
                        NSWorkspace.shared.activateFileViewerSelecting([URL(fileURLWithPath: i.path)])
                    } label: {
                        Image(systemName: "folder")
                    }
                    .buttonStyle(.borderless)
                    .foregroundStyle(.secondary)
                    .help("Show in Finder")
                }
            }
            if let note = i.note {
                Text(Format.inline(note)).font(.caption).foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
            if let s = suggestion, let sizes = s.sizes, sizes.count >= 2 {
                SizeBars(points: sizes, tint: s.tint)
                // A folder whose parts are listed on their own rows shows less
                // than the bars, which measure all of it.
                if let last = sizes.last?.bytes, Double(last) > Double(i.bytes) * 1.1 {
                    Text("The whole folder, with the parts listed on their own.")
                        .font(.caption2).foregroundStyle(.secondary)
                }
            }
            // Kept: no advice to make it smaller.
            if let about = i.about, !i.has("kept") {
                AboutBox(about: about, monitor: monitor)
            }
            if fix || keep {
                HStack(alignment: .bottom, spacing: 12) {
                    if let s = suggestion {
                        FixActions(fix: s.fix, line: s.headline, action: s.action, monitor: monitor)
                    } else if let a = i.action {
                        #if APP_STORE
                        LiteCommand(action: a, monitor: monitor)
                        #else
                        CleanButton(action: a, command: nil, monitor: monitor)
                        #endif
                    }
                    Spacer(minLength: 0)
                    if keep {
                        KeepButton(path: i.path, monitor: monitor)
                    }
                }
            }
        }
        .padding(.vertical, 10)
    }
}

#if !APP_STORE
/// Which caches to empty, each with a checkbox, all checked to start: the
/// sheet is the confirmation, so nothing is asked again.
private struct CachePicker: View {
    let items: [SpaceReport.Item]
    @ObservedObject var monitor: Monitor
    @Environment(\.dismiss) private var dismiss
    @State private var chosen: Set<String> = []

    var body: some View {
        let picked = items.filter { chosen.contains($0.path) }
        let size = picked.reduce(0) { $0 + $1.bytes }
        VStack(alignment: .leading, spacing: 14) {
            Text("Empty which caches?").font(.title3.weight(.semibold))
            Text("What's in them is deleted, not moved to the Trash. Apps make what they need again, so some are slower for a while and download some of it again. One whose app is open is left until you quit the app.")
                .font(.callout).foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
            VStack(spacing: 0) {
                ForEach(Array(items.enumerated()), id: \.element.id) { i, item in
                    if i > 0 { Divider() }
                    Toggle(isOn: Binding(
                        get: { chosen.contains(item.path) },
                        set: { if $0 { chosen.insert(item.path) } else { chosen.remove(item.path) } }
                    )) {
                        HStack {
                            VStack(alignment: .leading, spacing: 1) {
                                Text(item.name).font(.callout.weight(.medium))
                                Text(Format.path(item.path)).font(.caption).foregroundStyle(.secondary)
                                    .lineLimit(1).truncationMode(.middle)
                            }
                            Spacer(minLength: 12)
                            Text(Format.bytes(item.bytes)).font(.callout).monospacedDigit()
                        }
                    }
                    .toggleStyle(.checkbox)
                    .padding(.vertical, 7)
                }
            }
            .padding(.horizontal, 12)
            .background(Color.secondary.opacity(0.06), in: RoundedRectangle(cornerRadius: 8))
            HStack {
                Button(chosen.count == items.count ? "Select None" : "Select All") {
                    chosen = chosen.count == items.count ? [] : Set(items.map(\.path))
                }
                .buttonStyle(.link)
                Spacer()
                Button("Cancel", role: .cancel) { dismiss() }
                    .keyboardShortcut(.cancelAction)
                Button(picked.isEmpty ? "Empty" : "Empty \(picked.count) (\(Format.bytes(size)))") {
                    monitor.clean(CleanAction(kind: "empty", label: "Empty Caches",
                                              paths: picked.flatMap { $0.action?.paths ?? [] },
                                              confirm: nil, done: nil))
                    dismiss()
                }
                .keyboardShortcut(.defaultAction)
                .disabled(picked.isEmpty)
            }
        }
        .padding(22)
        .frame(width: 520)
        .onAppear { chosen = Set(items.map(\.path)) }
    }
}
#endif

/// Suggestions that aren't about an item listed above, nor an incident
/// shown under Needs Attention.
struct MoreFixes: View {
    @ObservedObject var monitor: Monitor

    var body: some View {
        let list = monitor.unplacedSuggestions
        if !list.isEmpty {
            VStack(alignment: .leading, spacing: 12) {
                SectionHeader(title: "More fixes", subtitle: "What else would free space, or keep it from filling again.")
                ForEach(Array(list.enumerated()), id: \.offset) { _, s in
                    SuggestionCard(suggestion: s, monitor: monitor)
                }
            }
        }
    }
}

enum SpaceText {
    static func color(_ kind: String) -> Color {
        switch kind {
        case "macos": .gray
        case "apps": .blue
        case "build": .orange
        case "dev": .indigo
        case "cloud": .cyan
        case "vms": .purple
        case "media": .pink
        case "mail": .teal
        case "caches": .yellow
        case "appdata": .mint
        case "files": .green
        default: .secondary
        }
    }

    /// What's freed without a clean-up, by macOS's names: "Purgeable".
    static func label(_ w: String) -> String {
        switch w {
        case "purgeable": "Purgeable"
        case "swap": "Swap"
        case "update": "macOS update"
        case "trash": "In the Trash"
        default: w.capitalized
        }
    }

    /// The same, inside a sentence: "27 GB purgeable", "13 GB macOS update".
    static func phrase(_ w: String) -> String {
        w == "update" ? label(w) : label(w).lowercased()
    }

    /// The biggest of what looks forgotten, from every kind: unchanged or
    /// not opened for `days` at least, `min` bytes or more.
    static func forgotten(_ r: SpaceReport, min: Int64 = 500_000_000, days: Int = 365,
                          limit: Int = 6) -> [SpaceReport.Item] {
        let span = Int64(days) * 86_400
        let all = r.kinds.flatMap(\.items)
            .filter { i in
                // The Trash has its own button, in the sidebar.
                guard !i.has("kept"), i.bytes >= min, !i.path.hasSuffix("/.Trash") else { return false }
                if i.has("cold"), (i.age ?? 0) >= span { return true }
                return i.has("unused") && r.measured - (i.lastUsed ?? 0) >= span
            }
            .sorted { $0.bytes > $1.bytes }
        return Array(all.prefix(limit))
    }

    /// Whether an app says how to make it smaller, and it's worth a look:
    /// listed with its kind even past the first few.
    static func advised(_ i: SpaceReport.Item) -> Bool {
        (i.about?.bytes ?? 0) >= 500_000_000 && !i.has("kept")
    }

    /// A kind's caches that can be emptied, biggest first.
    static func emptiable(_ k: SpaceReport.Kind) -> [SpaceReport.Item] {
        k.items.filter { $0.action?.kind == "empty" }.sorted { $0.bytes > $1.bytes }
    }

    /// Whether it's marked as something that may go: worth a Keep It.
    static func mayGo(_ i: SpaceReport.Item) -> Bool {
        ["unused", "cold", "build", "cache"].contains { i.has($0) }
    }

    /// Its marks, as pills: text, colour, symbol.
    static func marks(_ i: SpaceReport.Item) -> [(String, Color, String?)] {
        var out: [(String, Color, String?)] = []
        if i.has("kept") { out.append(("Kept", .secondary, "pin.fill")) }
        if i.has("unused") {
            if let t = i.lastUsed {
                out.append(("Not opened in \(Format.age(Int64(Date().timeIntervalSince1970) - t))", .orange, "moon.zzz.fill"))
            } else {
                out.append(("Never opened", .orange, "moon.zzz.fill"))
            }
        }
        if i.has("cold"), let a = i.age {
            out.append(("Unchanged for \(Format.age(a))", .blue, "snowflake"))
        }
        if i.has("build") { out.append(("Build output", .orange, "hammer.fill")) }
        if i.has("cache") { out.append(("Cache", .yellow, "arrow.triangle.2.circlepath")) }
        return out
    }
}

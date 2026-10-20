// The disk map drawn two ways, with the same controls: a sunburst (rings out
// from the folder in focus) and a treemap (tiles sized by space, coloured like
// a market heatmap by how much each folder grew or shrank in the last day).
// Click a folder to zoom in; the breadcrumb or the centre goes back out.

import AppKit
import SwiftUI

enum MapStyle: String, CaseIterable, Identifiable {
    case sunburst = "Sunburst", treemap = "Treemap", list = "List"
    var id: String { rawValue }
}

enum MapColouring: String, CaseIterable, Identifiable {
    case growth = "Growth, last 24 hours", folder = "Folder"
    var id: String { rawValue }
}

/// What the charts need to know about a folder besides its size.
struct MapContext {
    /// Growth in the last 24 hours, summed over the watcher's owner folders
    /// at or below each path (paths as the user knows them).
    let growth: [String: Int64]

    init(growth24h: [ForecastDetail.Growth]) {
        var g: [String: Int64] = [:]
        for row in growth24h {
            var p = row.owner
            // Every ancestor gets the growth of what's below it.
            while true {
                g[p, default: 0] += row.bytes
                guard let cut = p.lastIndex(of: "/"), p != "/" else { break }
                p = cut == p.startIndex ? "/" : String(p[..<cut])
            }
        }
        growth = g
    }

    static func userPath(_ node: Node) -> String {
        let data = "/System/Volumes/Data"
        if node.path == data { return "/" }
        return node.path.hasPrefix(data + "/") ? String(node.path.dropFirst(data.count)) : node.path
    }

    func growth(of node: Node) -> Int64? { growth[Self.userPath(node)] }

    static func name(_ node: Node) -> String {
        let p = userPath(node)
        return p == "/" ? "Macintosh HD" : URL(fileURLWithPath: p).lastPathComponent
    }

    /// Red for what grew, green for what shrank, grey for steady or unwatched;
    /// stronger with more bytes (10 MB faint, 10 GB full).
    func colour(_ node: Node?, _ mode: MapColouring, hue: Double, depth: Int) -> Color {
        guard let node else { return Color.gray.opacity(0.25) }
        switch mode {
        case .folder:
            // Strong in the middle, lighter towards the edge.
            let d = Double(depth - 1)
            return Color(hue: hue, saturation: max(0.28, 0.62 - 0.1 * d), brightness: min(0.96, 0.84 + 0.04 * d))
        case .growth:
            guard let g = growth(of: node), abs(g) >= 10_000_000 else {
                return Color(white: 0.42 - 0.04 * Double(min(depth, 4)))
            }
            let t = min(1, log10(Double(abs(g)) / 1e7) / 3)
            return g > 0
                ? Color(red: 0.45 + 0.45 * t, green: 0.30 - 0.12 * t, blue: 0.30 - 0.12 * t)
                : Color(red: 0.22 - 0.1 * t, green: 0.45 + 0.35 * t, blue: 0.30 - 0.05 * t)
        }
    }

    /// Top-level folders take these in turn, starting from the brand's blue;
    /// none is red, which means growth in the other colouring.
    static func hue(_ index: Int) -> Double {
        let palette = [0.62, 0.50, 0.76, 0.90, 0.09, 0.40, 0.68, 0.55, 0.14, 0.83]
        return palette[index % palette.count]
    }
}

/// The explorer: style and colour pickers, breadcrumb, what's under the
/// pointer, and the chart.
struct MapExplorer: View {
    let map: DiskMap
    let context: MapContext
    @State private var style: MapStyle = .sunburst
    @State private var colouring: MapColouring = .folder
    @State private var trail: [Node] = []
    @State private var hovered: Node?

    private var focus: Node { trail.last ?? map.scan.root }

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack {
                Picker("", selection: $style) {
                    ForEach(MapStyle.allCases) { Text($0.rawValue).tag($0) }
                }
                .pickerStyle(.segmented).labelsHidden().frame(width: 260)
                if style != .list {
                    Picker("Colour", selection: $colouring) {
                        ForEach(MapColouring.allCases) { Text($0.rawValue).tag($0) }
                    }
                    .frame(width: 250)
                }
                Spacer()
                Button("Show in Finder") {
                    NSWorkspace.shared.activateFileViewerSelecting([URL(fileURLWithPath: focus.path)])
                }
                .controlSize(.small)
            }
            breadcrumb
            info
            Group {
                switch style {
                case .sunburst:
                    SunburstChart(focus: focus, context: context, colouring: colouring,
                                  hovered: $hovered, open: open, back: back)
                case .treemap:
                    TreemapChart(focus: focus, context: context, colouring: colouring,
                                 hovered: $hovered, open: open)
                case .list:
                    List(focus.children ?? [], children: \.children) { node in
                        ListRow(node: node, top: focus.totals.privateBytes, context: context)
                    }
                    .listStyle(.inset(alternatesRowBackgrounds: true))
                }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            if style != .list && colouring == .growth {
                Text("Red: grew in the last 24 hours. Green: shrank. Grey: steady, or not watched yet. Size is what deleting it would free.")
                    .font(.caption).foregroundStyle(.secondary)
            }
        }
    }

    private var breadcrumb: some View {
        HStack(spacing: 4) {
            let path = [map.scan.root] + trail
            ForEach(Array(path.enumerated()), id: \.offset) { i, node in
                if i > 0 { Image(systemName: "chevron.right").font(.caption2).foregroundStyle(.tertiary) }
                Button(MapContext.name(node)) { trail = Array(trail.prefix(i)) }
                    .buttonStyle(.link)
                    .disabled(i == path.count - 1)
            }
            Spacer()
        }
        .font(.callout)
    }

    /// What's under the pointer, or the folder in focus.
    private var info: some View {
        let node = hovered ?? focus
        let share = focus.totals.privateBytes > 0
            ? Double(node.totals.privateBytes) / Double(focus.totals.privateBytes) : 0
        var line = "\(Format.bytes(node.totals.privateBytes))"
        if node !== focus { line += String(format: " · %.0f%% of %@", share * 100, MapContext.name(focus)) }
        if let g = context.growth(of: node), abs(g) >= 1_000_000 {
            line += " · \(g > 0 ? "grew" : "shrank") \(Format.bytes(abs(g))) in 24 hours"
        }
        return HStack(spacing: 8) {
            Text(Format.path(node.path)).fontWeight(.medium).lineLimit(1).truncationMode(.middle)
            Text(line).foregroundStyle(.secondary).monospacedDigit()
            Spacer()
        }
        .font(.callout)
    }

    private func open(_ node: Node) {
        guard node.children != nil, node !== focus else { return }
        // Zoom through the folders between, so the breadcrumb stays whole.
        var chain: [Node] = []
        var level = focus
        while level !== node, let next = level.children?.first(where: { node.path.hasPrefix($0.path) }) {
            chain.append(next)
            level = next
        }
        trail += chain
        hovered = nil
    }

    private func back() {
        if !trail.isEmpty { trail.removeLast() }
        hovered = nil
    }
}

// MARK: - Sunburst

struct Segment {
    /// nil: the files and small folders not shown on their own.
    let node: Node?
    let depth: Int
    /// Radians, clockwise from 12 o'clock.
    let start: Double
    let end: Double
    let hue: Double
}

struct SunburstChart: View {
    let focus: Node
    let context: MapContext
    let colouring: MapColouring
    @Binding var hovered: Node?
    let open: (Node) -> Void
    let back: () -> Void

    static let rings = 4

    var body: some View {
        GeometryReader { geo in
            let size = geo.size
            let centre = CGPoint(x: size.width / 2, y: size.height / 2)
            let radius = min(size.width, size.height) / 2 - 4
            let core = radius * 0.24
            let ring = (radius - core) / Double(Self.rings)
            let segments = Self.segments(focus)
            Canvas { ctx, _ in
                for s in segments {
                    let inner = core + ring * Double(s.depth - 1)
                    let path = Self.wedge(centre, inner + 0.5, inner + ring - 0.5, s.start, s.end)
                    var fill = context.colour(s.node, colouring, hue: s.hue, depth: s.depth)
                    if let h = hovered, let n = s.node, n === h { fill = fill.opacity(0.7) }
                    ctx.fill(path, with: .color(fill))
                    ctx.stroke(path, with: .color(Theme.card), lineWidth: 1.5)
                    // Names where they fit: on the first two rings, wide arcs.
                    let mid = (s.start + s.end) / 2, r = inner + ring / 2
                    let arc = (s.end - s.start) * r
                    if let n = s.node, s.depth <= 2, arc > 46 {
                        let p = CGPoint(x: centre.x + r * sin(mid), y: centre.y - r * cos(mid))
                        ctx.draw(Text(MapContext.name(n)).font(.system(size: 10, weight: .medium))
                                    .foregroundColor(.white), at: p)
                    }
                }
                let disc = Path(ellipseIn: CGRect(x: centre.x - core, y: centre.y - core, width: 2 * core, height: 2 * core))
                ctx.fill(disc, with: .color(Color.primary.opacity(0.06)))
                ctx.draw(Text(MapContext.name(focus)).font(.headline), at: CGPoint(x: centre.x, y: centre.y - 9))
                ctx.draw(Text(Format.bytes(focus.totals.privateBytes)).font(.callout).foregroundColor(.secondary),
                         at: CGPoint(x: centre.x, y: centre.y + 10))
            }
            .contentShape(Rectangle())
            .onContinuousHover { phase in
                switch phase {
                case .active(let p):
                    hovered = Self.hit(p, centre, core, ring, segments)?.node
                case .ended:
                    hovered = nil
                }
            }
            .onTapGesture { p in
                if hypot(p.x - centre.x, p.y - centre.y) < core {
                    back()
                } else if let n = Self.hit(p, centre, core, ring, segments)?.node {
                    open(n)
                }
            }
        }
        .help("Click a ring to zoom in, the centre to go back out")
    }

    static func segments(_ focus: Node) -> [Segment] {
        var out: [Segment] = []
        func add(_ node: Node, depth: Int, _ start: Double, _ end: Double, hue: Double?) {
            let total = Double(max(node.totals.privateBytes, 1))
            var at = start
            for (i, child) in (node.children ?? []).enumerated() {
                let span = (end - start) * Double(child.totals.privateBytes) / total
                guard span > 0.004 else { continue }
                let h = hue ?? MapContext.hue(i)
                out.append(Segment(node: child, depth: depth, start: at, end: at + span, hue: h))
                if depth < rings { add(child, depth: depth + 1, at, at + span, hue: h) }
                at += span
            }
            if end - at > 0.01 {
                out.append(Segment(node: nil, depth: depth, start: at, end: end, hue: 0))
            }
        }
        add(focus, depth: 1, 0, 2 * .pi, hue: nil)
        return out
    }

    /// An annular sector, as a polygon: no arc directions to get wrong.
    static func wedge(_ c: CGPoint, _ r0: Double, _ r1: Double, _ a0: Double, _ a1: Double) -> Path {
        let steps = max(2, Int((a1 - a0) / 0.03))
        let point = { (r: Double, a: Double) in CGPoint(x: c.x + r * sin(a), y: c.y - r * cos(a)) }
        var p = Path()
        p.move(to: point(r1, a0))
        for k in 1...steps { p.addLine(to: point(r1, a0 + (a1 - a0) * Double(k) / Double(steps))) }
        for k in stride(from: steps, through: 0, by: -1) {
            p.addLine(to: point(r0, a0 + (a1 - a0) * Double(k) / Double(steps)))
        }
        p.closeSubpath()
        return p
    }

    static func hit(_ p: CGPoint, _ c: CGPoint, _ core: Double, _ ring: Double,
                    _ segments: [Segment]) -> Segment? {
        let dx = p.x - c.x, dy = p.y - c.y
        let r = hypot(dx, dy)
        guard r >= core else { return nil }
        let depth = Int((r - core) / ring) + 1
        var a = atan2(dx, -dy)
        if a < 0 { a += 2 * .pi }
        return segments.first { $0.depth == depth && a >= $0.start && a < $0.end }
    }
}

// MARK: - Treemap

struct Tile {
    let node: Node?
    let rect: CGRect
    let depth: Int
    let hue: Double
    /// The top-level tile this one sits in.
    let group: Node?
}

struct TreemapChart: View {
    let focus: Node
    let context: MapContext
    let colouring: MapColouring
    @Binding var hovered: Node?
    let open: (Node) -> Void

    var body: some View {
        GeometryReader { geo in
            let tiles = Self.tiles(focus, in: CGRect(origin: .zero, size: geo.size))
            Canvas { ctx, _ in
                for t in tiles {
                    let r = t.rect.insetBy(dx: 1, dy: 1)
                    guard r.width > 1, r.height > 1 else { continue }
                    var fill = context.colour(t.node, colouring, hue: t.hue, depth: t.depth)
                    if let h = hovered, let n = t.node, n === h { fill = fill.opacity(0.7) }
                    if t.depth == 1 && t.node?.children != nil && r.width > 90 && r.height > 60 {
                        // A group: a header strip over its own tiles, as in a
                        // sector of a market heatmap.
                        ctx.fill(Path(roundedRect: r, cornerRadius: 5), with: .color(Color.primary.opacity(0.08)))
                        if let n = t.node {
                            label(ctx, n, in: CGRect(x: r.minX + 4, y: r.minY, width: r.width - 8, height: 16),
                                  header: true)
                        }
                    } else {
                        ctx.fill(Path(roundedRect: r, cornerRadius: 4), with: .color(fill))
                        if let n = t.node { label(ctx, n, in: r, header: false) }
                    }
                }
            }
            .contentShape(Rectangle())
            .onContinuousHover { phase in
                switch phase {
                case .active(let p): hovered = Self.hit(p, tiles)?.node
                case .ended: hovered = nil
                }
            }
            .onTapGesture { p in
                guard let t = Self.hit(p, tiles), let n = t.node else { return }
                // A folder with folders inside opens; a leaf opens its group.
                if n.children != nil { open(n) } else if let g = t.group { open(g) }
            }
        }
        .help("Click a tile to zoom in")
    }

    private func label(_ ctx: GraphicsContext, _ n: Node, in r: CGRect, header: Bool) {
        let name = MapContext.name(n), size = Format.bytes(n.totals.privateBytes)
        if header {
            ctx.draw(Text("\(name)  \(size)").font(.system(size: 10, weight: .semibold))
                        .foregroundColor(.secondary),
                     in: r)
            return
        }
        guard r.width > 54, r.height > 28 else { return }
        let big = min(max(10, min(r.width / 8, r.height / 3.2)), 22)
        var lines: [(String, CGFloat, Font.Weight)] = [(name, big, .semibold), (size, big * 0.72, .regular)]
        if let g = context.growth(of: n), abs(g) >= 10_000_000, r.height > 3.6 * big {
            lines.append(((g > 0 ? "+" : "−") + Format.bytes(abs(g)) + " in 24 h", big * 0.62, .regular))
        }
        let height = lines.reduce(0) { $0 + $1.1 * 1.25 }
        var y = r.midY - height / 2
        for (text, size, weight) in lines {
            // Roughly what fits: about 0.55 em a character.
            let fits = max(3, Int(r.width / (size * 0.55)))
            let shown = text.count > fits ? String(text.prefix(fits - 1)) + "…" : text
            ctx.draw(Text(shown).font(.system(size: size, weight: weight))
                        .foregroundColor(.white.opacity(weight == .semibold ? 1 : 0.85)),
                     at: CGPoint(x: r.midX, y: y + size * 0.62))
            y += size * 1.25
        }
    }

    static func tiles(_ focus: Node, in rect: CGRect) -> [Tile] {
        var out: [Tile] = []
        let top = Squarify.layout(items(focus), in: rect)
        for (i, (node, r)) in top.enumerated() {
            let hue = MapContext.hue(i)
            out.append(Tile(node: node, rect: r, depth: 1, hue: hue, group: node))
            // One level inside each tile big enough to show it.
            guard let node, node.children != nil, r.width > 90, r.height > 60 else { continue }
            let inner = CGRect(x: r.minX + 2, y: r.minY + 17, width: r.width - 4, height: r.height - 19)
            for (sub, sr) in Squarify.layout(items(node), in: inner) {
                out.append(Tile(node: sub, rect: sr, depth: 2, hue: hue, group: node))
            }
        }
        return out
    }

    /// Children by size, and what's left (files, small folders) as one item.
    private static func items(_ node: Node) -> [(Node?, Double)] {
        var list: [(Node?, Double)] = (node.children ?? []).map { ($0, Double($0.totals.privateBytes)) }
        let rest = Double(node.totals.privateBytes) - list.reduce(0) { $0 + $1.1 }
        if rest > Double(node.totals.privateBytes) * 0.01 { list.append((nil, rest)) }
        return list.filter { $0.1 > 0 }
    }

    private static func hit(_ p: CGPoint, _ tiles: [Tile]) -> Tile? {
        // The innermost tile under the point.
        tiles.last { $0.rect.contains(p) && ($0.depth == 2 || !tiles.contains { $0.depth == 2 && $0.rect.contains(p) }) }
    }
}

/// Squarified treemap layout (Bruls, Huizing, van Wijk): rows along the
/// shorter side, each row kept while it makes its tiles squarer.
enum Squarify {
    static func layout<T>(_ items: [(T, Double)], in rect: CGRect) -> [(T, CGRect)] {
        let sorted = items.sorted { $0.1 > $1.1 }
        let total = sorted.reduce(0) { $0 + $1.1 }
        guard total > 0, rect.width > 0, rect.height > 0 else { return [] }
        let scale = Double(rect.width * rect.height) / total
        var rest = rect
        var out: [(T, CGRect)] = []
        var row: [(T, Double)] = []
        var queue = sorted.map { ($0.0, $0.1 * scale) }[...]

        func worst(_ row: [(T, Double)], _ side: Double) -> Double {
            let s = row.reduce(0) { $0 + $1.1 }
            guard s > 0, let mx = row.map(\.1).max(), let mn = row.map(\.1).min() else { return .infinity }
            return max(side * side * mx / (s * s), (s * s) / (side * side * mn))
        }
        func place(_ row: [(T, Double)]) {
            let s = row.reduce(0) { $0 + $1.1 }
            if rest.width >= rest.height {
                let w = s / Double(rest.height)
                var y = rest.minY
                for (t, a) in row {
                    let h = a / w
                    out.append((t, CGRect(x: rest.minX, y: y, width: w, height: h)))
                    y += h
                }
                rest = CGRect(x: rest.minX + w, y: rest.minY, width: max(0, rest.width - w), height: rest.height)
            } else {
                let h = s / Double(rest.width)
                var x = rest.minX
                for (t, a) in row {
                    let w = a / h
                    out.append((t, CGRect(x: x, y: rest.minY, width: w, height: h)))
                    x += w
                }
                rest = CGRect(x: rest.minX, y: rest.minY + h, width: rest.width, height: max(0, rest.height - h))
            }
        }
        while let next = queue.first {
            let side = Double(min(rest.width, rest.height))
            if row.isEmpty || worst(row + [next], side) <= worst(row, side) {
                row.append(next)
                queue = queue.dropFirst()
            } else {
                place(row)
                row = []
            }
        }
        if !row.isEmpty { place(row) }
        return out
    }
}

// MARK: - List

private struct ListRow: View {
    let node: Node
    let top: Int64
    let context: MapContext

    var body: some View {
        let share = top > 0 ? Double(node.totals.privateBytes) / Double(top) : 0
        HStack(spacing: 10) {
            Image(systemName: "folder").foregroundStyle(.secondary)
            Text(MapContext.name(node)).lineLimit(1).truncationMode(.middle)
            Spacer()
            if let g = context.growth(of: node), abs(g) >= 10_000_000 {
                Text((g > 0 ? "+" : "−") + Format.bytes(abs(g)))
                    .font(.caption).monospacedDigit()
                    .foregroundStyle(g > 0 ? .red : .green)
            }
            Capsule().fill(LinearGradient(colors: Theme.brand, startPoint: .leading, endPoint: .trailing))
                .frame(width: max(2, 80 * share), height: 6)
                .frame(width: 80, alignment: .leading)
            Text(Format.bytes(node.totals.privateBytes))
                .monospacedDigit()
                .frame(width: 80, alignment: .trailing)
        }
        .help(Format.path(node.path))
    }
}

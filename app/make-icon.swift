// Draws the app icon: a drive under a gauge that is nearly full, its fill
// going from calm to alarm, and a dashed stretch for the forecast to full.
// The App Store edition's has a LITE badge under the drive.
//
//   swift app/make-icon.swift build/MyDiskIsFull.icon [--lite]
//   swift app/make-icon.swift build/MyDiskIsFull.iconset [--lite]
//
// An .icon is what Icon Composer makes: the artwork in layers, which actool
// compiles into the icon macOS 26 shapes and lights itself, and an .icns for
// earlier versions. Drawn on a rounded tile of its own, as the .iconset is,
// macOS 26 would put the icon in a grey frame. The .iconset is for when
// actool can't read an .icon (before Xcode 26): the tile on the Big Sur grid.

import AppKit

let S: CGFloat = 1024

/// Where the tile is on the 1024 canvas: inset on the Big Sur grid, or the
/// whole canvas for the layers, which macOS shapes.
struct Grid {
    let inset: CGFloat
    var side: CGFloat { S - inset * 2 }
    var rect: CGRect { CGRect(x: inset, y: inset, width: side, height: side) }
    var centre: CGPoint { CGPoint(x: S / 2, y: S / 2 + side * 0.01) }
}

func rgb(_ r: CGFloat, _ g: CGFloat, _ b: CGFloat, _ a: CGFloat = 1) -> NSColor {
    NSColor(srgbRed: r, green: g, blue: b, alpha: a)
}

func gradient(_ colors: [NSColor], _ locations: [CGFloat]) -> CGGradient {
    CGGradient(colorsSpace: CGColorSpace(name: CGColorSpace.sRGB),
               colors: colors.map(\.cgColor) as CFArray, locations: locations)!
}

/// Linear interpolation through evenly spaced colour stops.
func colour(at t: CGFloat, _ stops: [NSColor]) -> NSColor {
    let x = min(max(t, 0), 1) * CGFloat(stops.count - 1)
    let i = min(Int(x), stops.count - 2)
    return stops[i].blended(withFraction: x - CGFloat(i), of: stops[i + 1]) ?? stops[i]
}

/// Deep indigo into navy, a cool glow from the top left, and a glassy sheen
/// fading down the top half. Fills the tile's rectangle: the caller clips it.
func background(_ ctx: CGContext, _ g: Grid) {
    let (inset, side) = (g.inset, g.side)
    rgb(0.07, 0.09, 0.22).setFill()
    g.rect.fill()
    ctx.drawLinearGradient(gradient([rgb(0.16, 0.20, 0.48), rgb(0.06, 0.08, 0.22), rgb(0.03, 0.04, 0.12)], [0, 0.55, 1]),
                           start: CGPoint(x: 0, y: inset + side), end: CGPoint(x: 0, y: inset), options: [])
    ctx.drawRadialGradient(gradient([rgb(0.20, 0.80, 0.90, 0.38), rgb(0.20, 0.80, 0.90, 0)], [0, 1]),
                           startCenter: CGPoint(x: inset + side * 0.18, y: inset + side * 0.92), startRadius: 0,
                           endCenter: CGPoint(x: inset + side * 0.18, y: inset + side * 0.92), endRadius: side * 0.75,
                           options: [])
    ctx.drawLinearGradient(gradient([rgb(1, 1, 1, 0.09), rgb(1, 1, 1, 0)], [0, 1]),
                           start: CGPoint(x: 0, y: inset + side), end: CGPoint(x: 0, y: inset + side * 0.5), options: [])
}

/// The gauge: 270°, open at the bottom, with its track, the forecast in
/// dashes, the fill and a knob where it ends.
func gauge(_ ctx: CGContext, _ g: Grid) {
    let side = g.side, centre = g.centre
    let r = side * 0.33
    let width = side * 0.095
    let start: CGFloat = 225, sweep: CGFloat = 270
    let used: CGFloat = 0.80
    let stops = [rgb(0.18, 0.86, 0.88), rgb(0.27, 0.48, 1.0), rgb(0.60, 0.38, 0.98),
                 rgb(1.0, 0.55, 0.22), rgb(1.0, 0.24, 0.36)]
    func point(_ deg: CGFloat) -> CGPoint {
        CGPoint(x: centre.x + cos(deg * .pi / 180) * r, y: centre.y + sin(deg * .pi / 180) * r)
    }
    func arc(_ a: CGFloat, _ b: CGFloat, width w: CGFloat, cap: NSBezierPath.LineCapStyle) -> NSBezierPath {
        let p = NSBezierPath()
        p.appendArc(withCenter: centre, radius: r, startAngle: a, endAngle: b, clockwise: true)
        p.lineWidth = w
        p.lineCapStyle = cap
        return p
    }

    // The track.
    rgb(1, 1, 1, 0.09).setStroke()
    arc(start, start - sweep, width: width, cap: .round).stroke()

    // The forecast: dashes from the end of the fill to full.
    let dashes = arc(start - sweep * used - 9, start - sweep, width: width * 0.32, cap: .round)
    dashes.setLineDash([0, width * 0.62], count: 2, phase: 0)
    rgb(1, 1, 1, 0.55).setStroke()
    dashes.stroke()

    // The fill, as many short strokes so its colour runs along the arc,
    // with a glow under it.
    ctx.saveGState()
    ctx.setShadow(offset: .zero, blur: side * 0.053, color: rgb(1.0, 0.45, 0.30, 0.55).cgColor)
    ctx.beginTransparencyLayer(auxiliaryInfo: nil)
    let steps = 240
    for k in 0..<steps {
        let t0 = CGFloat(k) / CGFloat(steps), t1 = CGFloat(k + 1) / CGFloat(steps)
        colour(at: (t0 + t1) / 2, stops).setStroke()
        // Round caps on the first and last strokes only, so the ends are
        // round and the joins are seamless.
        let cap: NSBezierPath.LineCapStyle = k == 0 || k == steps - 1 ? .round : .butt
        arc(start - sweep * used * t0, start - sweep * used * t1 - 0.4, width: width, cap: cap).stroke()
    }
    ctx.endTransparencyLayer()
    ctx.restoreGState()

    // Now: a bright knob where the fill ends.
    let now = point(start - sweep * used)
    ctx.saveGState()
    ctx.setShadow(offset: .zero, blur: side * 0.029, color: rgb(1, 1, 1, 0.8).cgColor)
    rgb(1, 1, 1).setFill()
    let knob = width * 0.42
    NSBezierPath(ovalIn: CGRect(x: now.x - knob / 2, y: now.y - knob / 2, width: knob, height: knob)).fill()
    ctx.restoreGState()
}

/// The drive, in the middle: a slab with a seam and a light.
func drive(_ ctx: CGContext, _ g: Grid) {
    let side = g.side, centre = g.centre
    let dw = side * 0.34, dh = side * 0.15
    let drive = CGRect(x: centre.x - dw / 2, y: centre.y - dh / 2 - side * 0.02, width: dw, height: dh)
    let slab = NSBezierPath(roundedRect: drive, xRadius: dh * 0.28, yRadius: dh * 0.28)
    ctx.saveGState()
    ctx.setShadow(offset: CGSize(width: 0, height: -side * 0.01), blur: side * 0.022, color: rgb(0, 0, 0, 0.5).cgColor)
    rgb(0.85, 0.88, 0.95).setFill()
    slab.fill()
    ctx.restoreGState()
    ctx.saveGState()
    slab.addClip()
    ctx.drawLinearGradient(gradient([rgb(0.98, 0.99, 1.0), rgb(0.74, 0.78, 0.88)], [0, 1]),
                           start: CGPoint(x: 0, y: drive.maxY), end: CGPoint(x: 0, y: drive.minY), options: [])
    ctx.restoreGState()
    rgb(0.45, 0.50, 0.62, 0.55).setFill()
    NSBezierPath(rect: CGRect(x: drive.minX + dh * 0.3, y: drive.midY - side * 0.003,
                              width: dw * 0.52, height: side * 0.006)).fill()
    let led = dh * 0.22
    ctx.saveGState()
    ctx.setShadow(offset: .zero, blur: side * 0.019, color: rgb(1.0, 0.45, 0.25, 0.9).cgColor)
    rgb(1.0, 0.42, 0.26).setFill()
    NSBezierPath(ovalIn: CGRect(x: drive.maxX - dh * 0.32 - led, y: drive.midY - led / 2, width: led, height: led)).fill()
    ctx.restoreGState()
}

/// LITE, white on the gauge's first colours, in the gap at the bottom of the
/// gauge: as large as fits there, to read in the Dock.
func badge(_ ctx: CGContext, _ g: Grid) {
    let side = g.side
    let font = NSFont.systemFont(ofSize: side * 0.105, weight: .heavy)
    let rounded = font.fontDescriptor.withDesign(.rounded).flatMap { NSFont(descriptor: $0, size: font.pointSize) } ?? font
    let kern = side * 0.008
    let text = NSAttributedString(string: "LITE", attributes: [
        .font: rounded, .foregroundColor: NSColor.white, .kern: kern,
    ])
    let size = text.size()
    let h = side * 0.15, w = size.width + h * 0.8
    let pill = CGRect(x: S / 2 - w / 2, y: g.inset + side * 0.065, width: w, height: h)
    let path = NSBezierPath(roundedRect: pill, xRadius: h / 2, yRadius: h / 2)
    ctx.saveGState()
    ctx.setShadow(offset: CGSize(width: 0, height: -side * 0.007), blur: side * 0.022, color: rgb(0, 0, 0, 0.45).cgColor)
    rgb(0.18, 0.62, 0.94).setFill()
    path.fill()
    ctx.restoreGState()
    ctx.saveGState()
    path.addClip()
    ctx.drawLinearGradient(gradient([rgb(0.18, 0.86, 0.88), rgb(0.27, 0.48, 1.0)], [0, 1]),
                           start: CGPoint(x: pill.minX, y: 0), end: CGPoint(x: pill.maxX, y: 0), options: [])
    ctx.restoreGState()
    // The kern after the last letter would push it off centre.
    text.draw(at: CGPoint(x: pill.midX - (size.width - kern) / 2, y: pill.midY - size.height / 2 + side * 0.002))
}

/// A 1024 canvas, drawn on.
func canvas(_ draw: (CGContext) -> Void) -> NSBitmapImageRep {
    let rep = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: Int(S), pixelsHigh: Int(S), bitsPerSample: 8,
                               samplesPerPixel: 4, hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB,
                               bytesPerRow: 0, bitsPerPixel: 0)!
    NSGraphicsContext.saveGraphicsState()
    let g = NSGraphicsContext(bitmapImageRep: rep)!
    NSGraphicsContext.current = g
    draw(g.cgContext)
    NSGraphicsContext.restoreGraphicsState()
    return rep
}

/// The whole icon on its own tile, for the .iconset.
func makeIcon(lite: Bool) -> NSBitmapImageRep {
    let g = Grid(inset: 100)
    let radius = g.side * 0.2237
    return canvas { ctx in
        let tile = NSBezierPath(roundedRect: g.rect, xRadius: radius, yRadius: radius)
        // A soft shadow under the tile.
        ctx.saveGState()
        ctx.setShadow(offset: CGSize(width: 0, height: -14), blur: 40, color: rgb(0, 0, 0, 0.45).cgColor)
        rgb(0.07, 0.09, 0.22).setFill()
        tile.fill()
        ctx.restoreGState()
        ctx.saveGState()
        tile.addClip()
        background(ctx, g)
        ctx.restoreGState()
        gauge(ctx, g)
        drive(ctx, g)
        if lite { badge(ctx, g) }
        // A hairline around the tile, lighter at the top.
        let edge = NSBezierPath(roundedRect: g.rect.insetBy(dx: 1.5, dy: 1.5), xRadius: radius - 1.5, yRadius: radius - 1.5)
        edge.lineWidth = 3
        rgb(1, 1, 1, 0.10).setStroke()
        edge.stroke()
    }
}

func png(_ rep: NSBitmapImageRep) -> Data { rep.representation(using: .png, properties: [:])! }

func iconset(_ out: String, lite: Bool) throws {
    let icon = NSImage(size: NSSize(width: S, height: S))
    icon.addRepresentation(makeIcon(lite: lite))
    let specs: [(String, Int)] = [
        ("icon_16x16", 16), ("icon_16x16@2x", 32), ("icon_32x32", 32), ("icon_32x32@2x", 64),
        ("icon_128x128", 128), ("icon_128x128@2x", 256), ("icon_256x256", 256),
        ("icon_256x256@2x", 512), ("icon_512x512", 512), ("icon_512x512@2x", 1024),
    ]
    for (name, px) in specs {
        let target = NSImage(size: NSSize(width: px, height: px))
        target.lockFocus()
        NSGraphicsContext.current?.imageInterpolation = .high
        icon.draw(in: NSRect(x: 0, y: 0, width: px, height: px))
        target.unlockFocus()
        guard let tiff = target.tiffRepresentation, let rep = NSBitmapImageRep(data: tiff) else { continue }
        rep.size = NSSize(width: px, height: px)
        try rep.representation(using: .png, properties: [:])?.write(to: URL(fileURLWithPath: "\(out)/\(name).png"))
    }
}

/// The layers, back to front, full bleed, and what Icon Composer would
/// write for them. The background is a layer of its own, not glass.
func layered(_ out: String, lite: Bool) throws {
    let g = Grid(inset: 0)
    var layers: [(String, NSBitmapImageRep)] = [
        ("background", canvas { background($0, g) }),
        ("gauge", canvas { gauge($0, g) }),
        ("drive", canvas { drive($0, g) }),
    ]
    if lite { layers.append(("lite", canvas { badge($0, g) })) }
    let assets = "\(out)/Assets"
    try FileManager.default.createDirectory(atPath: assets, withIntermediateDirectories: true)
    for (name, rep) in layers {
        try png(rep).write(to: URL(fileURLWithPath: "\(assets)/\(name).png"))
    }
    // Icon Composer lists groups front to back. Glass lets the background
    // through what's on it: not through LITE, which has to read.
    let groups: [[String: Any]] = layers.reversed().map { name, _ in
        var group: [String: Any] = ["layers": [["image-name": "\(name).png", "name": name]]]
        if name == "background" {
            group["specular"] = false
            group["shadow"] = ["kind": "none", "opacity": 0]
        }
        if name == "background" || name == "lite" {
            group["translucency"] = ["enabled": false, "value": 0]
        }
        return group
    }
    let json: [String: Any] = [
        "groups": groups,
        "supported-platforms": ["squares": ["macOS"]],
    ]
    let data = try JSONSerialization.data(withJSONObject: json, options: [.prettyPrinted, .sortedKeys])
    try data.write(to: URL(fileURLWithPath: "\(out)/icon.json"))
}

let args = CommandLine.arguments.dropFirst()
let out = args.first { !$0.hasPrefix("--") } ?? "build/MyDiskIsFull.icon"
let lite = args.contains("--lite")
try FileManager.default.createDirectory(atPath: out, withIntermediateDirectories: true)
if out.hasSuffix(".icon") {
    try layered(out, lite: lite)
} else {
    try iconset(out, lite: lite)
}

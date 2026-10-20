// Draws the app icon into an .iconset that iconutil turns into
// MyDiskIsFull.icns: a drive under a gauge that is nearly full, its fill
// going from calm to alarm, and a dashed stretch for the forecast to full.
// Canvas 1024; the rounded square sits on the Big Sur icon grid.
//
//   swift app/make-icon.swift build/MyDiskIsFull.iconset

import AppKit

let S: CGFloat = 1024
let inset: CGFloat = 100
let side = S - inset * 2
let radius = side * 0.2237

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

func makeIcon() -> NSImage {
    let img = NSImage(size: NSSize(width: S, height: S))
    img.lockFocus()
    guard let ctx = NSGraphicsContext.current?.cgContext else { img.unlockFocus(); return img }
    let rect = CGRect(x: inset, y: inset, width: side, height: side)
    let tile = NSBezierPath(roundedRect: rect, xRadius: radius, yRadius: radius)

    // The tile: deep indigo into navy, a cool glow from the top left, and a
    // soft shadow under it.
    ctx.saveGState()
    ctx.setShadow(offset: CGSize(width: 0, height: -14), blur: 40, color: rgb(0, 0, 0, 0.45).cgColor)
    rgb(0.07, 0.09, 0.22).setFill()
    tile.fill()
    ctx.restoreGState()

    ctx.saveGState()
    tile.addClip()
    ctx.drawLinearGradient(gradient([rgb(0.16, 0.20, 0.48), rgb(0.06, 0.08, 0.22), rgb(0.03, 0.04, 0.12)], [0, 0.55, 1]),
                           start: CGPoint(x: 0, y: inset + side), end: CGPoint(x: 0, y: inset), options: [])
    ctx.drawRadialGradient(gradient([rgb(0.20, 0.80, 0.90, 0.38), rgb(0.20, 0.80, 0.90, 0)], [0, 1]),
                           startCenter: CGPoint(x: inset + side * 0.18, y: inset + side * 0.92), startRadius: 0,
                           endCenter: CGPoint(x: inset + side * 0.18, y: inset + side * 0.92), endRadius: side * 0.75,
                           options: [])
    // A glassy sheen, fading down the top half.
    ctx.drawLinearGradient(gradient([rgb(1, 1, 1, 0.09), rgb(1, 1, 1, 0)], [0, 1]),
                           start: CGPoint(x: 0, y: inset + side), end: CGPoint(x: 0, y: inset + side * 0.5), options: [])
    ctx.restoreGState()

    // The gauge: 270°, open at the bottom.
    let centre = CGPoint(x: S / 2, y: S / 2 + side * 0.01)
    let r = side * 0.33
    let width = side * 0.095
    let start: CGFloat = 225, sweep: CGFloat = 270
    let used: CGFloat = 0.80
    let stops = [rgb(0.18, 0.86, 0.88), rgb(0.27, 0.48, 1.0), rgb(0.60, 0.38, 0.98),
                 rgb(1.0, 0.55, 0.22), rgb(1.0, 0.24, 0.36)]
    func point(_ deg: CGFloat, _ radius: CGFloat = r) -> CGPoint {
        CGPoint(x: centre.x + cos(deg * .pi / 180) * radius, y: centre.y + sin(deg * .pi / 180) * radius)
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
    ctx.setShadow(offset: .zero, blur: 44, color: rgb(1.0, 0.45, 0.30, 0.55).cgColor)
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
    ctx.setShadow(offset: .zero, blur: 24, color: rgb(1, 1, 1, 0.8).cgColor)
    rgb(1, 1, 1).setFill()
    let knob = width * 0.42
    NSBezierPath(ovalIn: CGRect(x: now.x - knob / 2, y: now.y - knob / 2, width: knob, height: knob)).fill()
    ctx.restoreGState()

    // The drive, in the middle: a slab with a light.
    let dw = side * 0.34, dh = side * 0.15
    let drive = CGRect(x: centre.x - dw / 2, y: centre.y - dh / 2 - side * 0.02, width: dw, height: dh)
    let slab = NSBezierPath(roundedRect: drive, xRadius: dh * 0.28, yRadius: dh * 0.28)
    ctx.saveGState()
    ctx.setShadow(offset: CGSize(width: 0, height: -8), blur: 18, color: rgb(0, 0, 0, 0.5).cgColor)
    rgb(0.85, 0.88, 0.95).setFill()
    slab.fill()
    ctx.restoreGState()
    ctx.saveGState()
    slab.addClip()
    ctx.drawLinearGradient(gradient([rgb(0.98, 0.99, 1.0), rgb(0.74, 0.78, 0.88)], [0, 1]),
                           start: CGPoint(x: 0, y: drive.maxY), end: CGPoint(x: 0, y: drive.minY), options: [])
    ctx.restoreGState()
    // Its seam and its light.
    rgb(0.45, 0.50, 0.62, 0.55).setFill()
    NSBezierPath(rect: CGRect(x: drive.minX + dh * 0.3, y: drive.midY - side * 0.003,
                              width: dw * 0.52, height: side * 0.006)).fill()
    let led = dh * 0.22
    ctx.saveGState()
    ctx.setShadow(offset: .zero, blur: 16, color: rgb(1.0, 0.45, 0.25, 0.9).cgColor)
    rgb(1.0, 0.42, 0.26).setFill()
    NSBezierPath(ovalIn: CGRect(x: drive.maxX - dh * 0.32 - led, y: drive.midY - led / 2, width: led, height: led)).fill()
    ctx.restoreGState()

    // A hairline around the tile, lighter at the top.
    ctx.saveGState()
    let edge = NSBezierPath(roundedRect: rect.insetBy(dx: 1.5, dy: 1.5), xRadius: radius - 1.5, yRadius: radius - 1.5)
    edge.lineWidth = 3
    rgb(1, 1, 1, 0.10).setStroke()
    edge.stroke()
    ctx.restoreGState()

    img.unlockFocus()
    return img
}

func png(_ img: NSImage, _ size: Int) -> Data? {
    let target = NSImage(size: NSSize(width: size, height: size))
    target.lockFocus()
    NSGraphicsContext.current?.imageInterpolation = .high
    img.draw(in: NSRect(x: 0, y: 0, width: size, height: size))
    target.unlockFocus()
    guard let tiff = target.tiffRepresentation, let rep = NSBitmapImageRep(data: tiff) else { return nil }
    rep.size = NSSize(width: size, height: size)
    return rep.representation(using: .png, properties: [:])
}

let out = CommandLine.arguments.count > 1 ? CommandLine.arguments[1] : "build/MyDiskIsFull.iconset"
try? FileManager.default.createDirectory(atPath: out, withIntermediateDirectories: true)
let icon = makeIcon()
let specs: [(String, Int)] = [
    ("icon_16x16", 16), ("icon_16x16@2x", 32), ("icon_32x32", 32), ("icon_32x32@2x", 64),
    ("icon_128x128", 128), ("icon_128x128@2x", 256), ("icon_256x256", 256),
    ("icon_256x256@2x", 512), ("icon_512x512", 512), ("icon_512x512@2x", 1024),
]
for (name, px) in specs {
    guard let d = png(icon, px) else { continue }
    try? d.write(to: URL(fileURLWithPath: "\(out)/\(name).png"))
}

// Draws the app icon into an .iconset that iconutil turns into
// MyDiskIsFull.icns: a gauge that is nearly full, the one thing the app is
// about. Canvas 1024; the rounded square sits on the Big Sur icon grid.
//
//   swift app/make-icon.swift build/MyDiskIsFull.iconset

import AppKit

let S: CGFloat = 1024
let inset: CGFloat = 100
let side = S - inset * 2
let radius = side * 0.2237

func makeIcon() -> NSImage {
    let img = NSImage(size: NSSize(width: S, height: S))
    img.lockFocus()
    guard let ctx = NSGraphicsContext.current?.cgContext else { img.unlockFocus(); return img }

    let rect = CGRect(x: inset, y: inset, width: side, height: side)
    ctx.saveGState()
    NSBezierPath(roundedRect: rect, xRadius: radius, yRadius: radius).addClip()
    let colors = [NSColor(srgbRed: 0.10, green: 0.20, blue: 0.27, alpha: 1).cgColor,
                  NSColor(srgbRed: 0.04, green: 0.08, blue: 0.11, alpha: 1).cgColor] as CFArray
    if let g = CGGradient(colorsSpace: CGColorSpaceCreateDeviceRGB(), colors: colors, locations: [0, 1]) {
        ctx.drawLinearGradient(g, start: CGPoint(x: 0, y: S), end: CGPoint(x: 0, y: 0), options: [])
    }
    ctx.restoreGState()

    // A 270° gauge, open at the bottom: the track, then the used part.
    let centre = CGPoint(x: S / 2, y: S / 2 - side * 0.02)
    let r = side * 0.32
    let start: CGFloat = 225, end: CGFloat = -45
    func arc(from a: CGFloat, to b: CGFloat) -> NSBezierPath {
        let p = NSBezierPath()
        p.appendArc(withCenter: centre, radius: r, startAngle: a, endAngle: b, clockwise: true)
        p.lineWidth = side * 0.085
        p.lineCapStyle = .round
        return p
    }
    NSColor(white: 1, alpha: 0.12).setStroke()
    arc(from: start, to: end).stroke()

    let used: CGFloat = 0.86
    ctx.saveGState()
    ctx.setShadow(offset: .zero, blur: 36,
                  color: NSColor(srgbRed: 1.0, green: 0.55, blue: 0.2, alpha: 0.8).cgColor)
    NSColor(srgbRed: 1.0, green: 0.62, blue: 0.22, alpha: 1).setStroke()
    arc(from: start, to: start - 270 * used).stroke()
    ctx.restoreGState()

    // The needle's hub and a short needle pointing at the end of the fill.
    let angle = (start - 270 * used) * .pi / 180
    let needle = NSBezierPath()
    needle.move(to: centre)
    needle.line(to: CGPoint(x: centre.x + cos(angle) * r * 0.72, y: centre.y + sin(angle) * r * 0.72))
    needle.lineWidth = side * 0.035
    needle.lineCapStyle = .round
    NSColor(white: 0.92, alpha: 1).setStroke()
    needle.stroke()
    let hub = side * 0.075
    NSColor(white: 0.92, alpha: 1).setFill()
    NSBezierPath(ovalIn: CGRect(x: centre.x - hub / 2, y: centre.y - hub / 2, width: hub, height: hub)).fill()

    img.unlockFocus()
    return img
}

func png(_ img: NSImage, _ size: Int) -> Data? {
    let target = NSImage(size: NSSize(width: size, height: size))
    target.lockFocus()
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

// label <text> <out.png> <height px> [background hex|-] [text hex] [font]: the
// text on a rounded plate, as a PNG for ffmpeg to overlay (this ffmpeg has no
// drawtext). The colours default to ux-kit's dark primary and its foreground;
// a colour may carry alpha (RRGGBBAA). Background "-" draws the text alone,
// tight to its bounds; font is a PostScript name (DINAlternate-Bold).
import AppKit

let args = CommandLine.arguments
let text = args[1] as NSString
let h = CGFloat(Double(args[3]) ?? 56)
func hex(_ i: Int, _ fallback: String) -> NSColor? {
    let s = args.count > i ? args[i] : fallback
    if s == "-" { return nil }
    let v = UInt32(s, radix: 16) ?? 0
    let rgba = s.count == 8 ? v : v << 8 | 0xff
    return NSColor(srgbRed: CGFloat(rgba >> 24 & 255) / 255, green: CGFloat(rgba >> 16 & 255) / 255,
                   blue: CGFloat(rgba >> 8 & 255) / 255, alpha: CGFloat(rgba & 255) / 255)
}
let plate = hex(4, "49a8b3")
let size0 = plate == nil ? h : h * 0.5
let font = (args.count > 6 ? NSFont(name: args[6], size: size0) : nil)
    ?? NSFont.systemFont(ofSize: size0, weight: .semibold)
let attrs: [NSAttributedString.Key: Any] = [.font: font, .foregroundColor: hex(5, "071417")!]
let size = text.size(withAttributes: attrs)
let w = plate == nil ? size.width.rounded(.up) : (size.width + h * 0.9).rounded()
let ht = plate == nil ? size.height.rounded(.up) : h
let rep = NSBitmapImageRep(
    bitmapDataPlanes: nil, pixelsWide: Int(w), pixelsHigh: Int(ht), bitsPerSample: 8,
    samplesPerPixel: 4, hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB,
    bytesPerRow: 0, bitsPerPixel: 0)!
rep.size = NSSize(width: w, height: ht) // one point per pixel, whatever the screen
NSGraphicsContext.saveGraphicsState()
NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)
if let plate {
    plate.setFill()
    NSBezierPath(roundedRect: NSRect(x: 0, y: 0, width: w, height: ht), xRadius: h * 0.22, yRadius: h * 0.22).fill()
}
text.draw(at: NSPoint(x: (w - size.width) / 2, y: (ht - size.height) / 2), withAttributes: attrs)
NSGraphicsContext.restoreGraphicsState()
try! rep.representation(using: .png, properties: [:])!.write(to: URL(fileURLWithPath: args[2]))

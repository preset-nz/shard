// label <text> <out.png> <height px> [background hex] [text hex]: the text on a
// rounded plate, as a PNG for ffmpeg to overlay (this ffmpeg has no drawtext).
// The colours default to ux-kit's dark primary and its foreground.
import AppKit

let args = CommandLine.arguments
let text = args[1] as NSString
let h = CGFloat(Double(args[3]) ?? 56)
func hex(_ i: Int, _ fallback: String) -> NSColor {
    let s = args.count > i ? args[i] : fallback
    let v = Int(s, radix: 16) ?? 0
    return NSColor(srgbRed: CGFloat((v >> 16) & 255) / 255, green: CGFloat((v >> 8) & 255) / 255,
                   blue: CGFloat(v & 255) / 255, alpha: 1)
}
let font = NSFont.systemFont(ofSize: h * 0.5, weight: .semibold)
let attrs: [NSAttributedString.Key: Any] = [.font: font, .foregroundColor: hex(5, "071417")]
let size = text.size(withAttributes: attrs)
let w = (size.width + h * 0.9).rounded()
let rep = NSBitmapImageRep(
    bitmapDataPlanes: nil, pixelsWide: Int(w), pixelsHigh: Int(h), bitsPerSample: 8,
    samplesPerPixel: 4, hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB,
    bytesPerRow: 0, bitsPerPixel: 0)!
rep.size = NSSize(width: w, height: h) // one point per pixel, whatever the screen
NSGraphicsContext.saveGraphicsState()
NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)
hex(4, "49a8b3").setFill()
NSBezierPath(roundedRect: NSRect(x: 0, y: 0, width: w, height: h), xRadius: h * 0.22, yRadius: h * 0.22).fill()
text.draw(at: NSPoint(x: (w - size.width) / 2, y: (h - size.height) / 2), withAttributes: attrs)
NSGraphicsContext.restoreGraphicsState()
try! rep.representation(using: .png, properties: [:])!.write(to: URL(fileURLWithPath: args[2]))

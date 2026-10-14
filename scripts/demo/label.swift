// label <text> <out.png> <height px>: the text, light on a dark rounded plate,
// as a PNG for ffmpeg to overlay. This ffmpeg has no drawtext.
import AppKit

let args = CommandLine.arguments
let text = args[1] as NSString
let h = CGFloat(Double(args[3]) ?? 56)
let font = NSFont.systemFont(ofSize: h * 0.55, weight: .semibold)
let attrs: [NSAttributedString.Key: Any] = [
    .font: font, .foregroundColor: NSColor(white: 0.92, alpha: 1),
]
let size = text.size(withAttributes: attrs)
let w = (size.width + h * 0.7).rounded()
let rep = NSBitmapImageRep(
    bitmapDataPlanes: nil, pixelsWide: Int(w), pixelsHigh: Int(h), bitsPerSample: 8,
    samplesPerPixel: 4, hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB,
    bytesPerRow: 0, bitsPerPixel: 0)!
NSGraphicsContext.saveGraphicsState()
NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)
NSColor(white: 0.08, alpha: 0.72).setFill()
NSBezierPath(roundedRect: NSRect(x: 0, y: 0, width: w, height: h), xRadius: h * 0.25, yRadius: h * 0.25).fill()
text.draw(at: NSPoint(x: (w - size.width) / 2, y: (h - size.height) / 2), withAttributes: attrs)
NSGraphicsContext.restoreGraphicsState()
try! rep.representation(using: .png, properties: [:])!.write(to: URL(fileURLWithPath: args[2]))

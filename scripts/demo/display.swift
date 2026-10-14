// display               the main display's mode as "w h pixelW pixelH" (points, then pixels)
// display <w> <h>       switch to the 2× mode of w×h points, at the current refresh
//                       rate where it has one. For this login only: a logout restores it.
//
// A 1920×1200 window doesn't fit a 1728×1117 screen; at 2056×1329 ("More
// Space") it does, and records at 3840×2400, which halves cleanly.
import CoreGraphics
import Foundation

let args = CommandLine.arguments
let d = CGMainDisplayID()
guard let cur = CGDisplayCopyDisplayMode(d) else { exit(1) }

if args.count >= 3, let w = Int(args[1]), let h = Int(args[2]) {
    let opts = [kCGDisplayShowDuplicateLowResolutionModes: true] as CFDictionary
    let modes = (CGDisplayCopyAllDisplayModes(d, opts) as? [CGDisplayMode] ?? [])
        .filter { $0.width == w && $0.height == h && $0.pixelWidth == w * 2 && $0.isUsableForDesktopGUI() }
    guard let mode = modes.first(where: { $0.refreshRate == cur.refreshRate })
        ?? modes.max(by: { $0.refreshRate < $1.refreshRate })
    else { FileHandle.standardError.write("no 2× mode of \(w)×\(h)\n".data(using: .utf8)!); exit(1) }
    if mode.width != cur.width || mode.height != cur.height || mode.pixelWidth != cur.pixelWidth {
        var cfg: CGDisplayConfigRef?
        guard CGBeginDisplayConfiguration(&cfg) == .success else { exit(1) }
        CGConfigureDisplayWithDisplayMode(cfg, d, mode, nil)
        guard CGCompleteDisplayConfiguration(cfg, .forSession) == .success else {
            FileHandle.standardError.write("the switch failed\n".data(using: .utf8)!)
            exit(1)
        }
    }
}
let m = CGDisplayCopyDisplayMode(d)!
print(m.width, m.height, m.pixelWidth, m.pixelHeight)

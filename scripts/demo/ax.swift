// Finds Shard's controls by what the accessibility tree calls them, so a demo
// script clicks a named control rather than a pixel read off a screenshot.
//
//   ax dump [filter]      every element: role, name, value, frame (points)
//   ax find <name> [n]    the centre of the n-th (from 1) element whose name
//                         or value contains <name>, as "x y w h"
//   ax next <name> <role> [n]  the first <role> (AXSlider, AXButton) after the
//                         n-th element named <name>: a slider after its label
//   ax slider <label> [n] the slider after the n-th (default last) text exactly <label>, as
//                         "thumbX y trackX trackWidth", to drag to a fraction
//
// Names are matched without case against title, description, label and value.
import AppKit
import ApplicationServices

let args = CommandLine.arguments
guard let app = NSWorkspace.shared.runningApplications.first(where: {
    $0.localizedName == "shard" || $0.bundleIdentifier == "nz.preset.shard"
}) else {
    FileHandle.standardError.write("shard isn't running\n".data(using: .utf8)!)
    exit(1)
}
let root = AXUIElementCreateApplication(app.processIdentifier)

func attr(_ e: AXUIElement, _ name: String) -> AnyObject? {
    var v: AnyObject?
    return AXUIElementCopyAttributeValue(e, name as CFString, &v) == .success ? v : nil
}
func text(_ e: AXUIElement, _ name: String) -> String {
    switch attr(e, name) {
    case let s as String: return s
    case let n as NSNumber: return n.stringValue
    default: return ""
    }
}
func frame(_ e: AXUIElement) -> CGRect? {
    guard let p = attr(e, kAXPositionAttribute), let s = attr(e, kAXSizeAttribute) else { return nil }
    var pt = CGPoint.zero, sz = CGSize.zero
    AXValueGetValue(p as! AXValue, .cgPoint, &pt)
    AXValueGetValue(s as! AXValue, .cgSize, &sz)
    return CGRect(origin: pt, size: sz)
}

struct Found { let role: String; let name: String; let value: String; let frame: CGRect }
var all: [Found] = []
func walk(_ e: AXUIElement, depth: Int) {
    if depth > 60 { return }
    let role = text(e, kAXRoleAttribute)
    let name = [kAXTitleAttribute, kAXDescriptionAttribute, "AXLabel", kAXHelpAttribute]
        .map { text(e, $0) }.filter { !$0.isEmpty }.joined(separator: " | ")
    if let f = frame(e), f.width > 0 {
        all.append(Found(role: role, name: name, value: text(e, kAXValueAttribute), frame: f))
    }
    for c in (attr(e, kAXChildrenAttribute) as? [AXUIElement]) ?? [] { walk(c, depth: depth + 1) }
}
walk(root, depth: 0)

func matches(_ f: Found, _ q: String) -> Bool {
    f.name.localizedCaseInsensitiveContains(q) || f.value.localizedCaseInsensitiveContains(q)
}

switch args.count > 1 ? args[1] : "dump" {
case "find":
    // An element named exactly that ("Drive") before one whose words merely
    // contain it ("A driven four-pole low-pass").
    let exact = all.filter {
        $0.name.components(separatedBy: " | ")[0].caseInsensitiveCompare(args[2]) == .orderedSame
            || $0.value.trimmingCharacters(in: .whitespacesAndNewlines).caseInsensitiveCompare(args[2]) == .orderedSame
    }
    let hits = exact.isEmpty ? all.filter { matches($0, args[2]) } : exact
    let n = args.count > 3 ? Int(args[3])! : 1
    guard hits.count >= n else { FileHandle.standardError.write("no \(args[2])\n".data(using: .utf8)!); exit(1) }
    let f = hits[n - 1].frame
    print(Int(f.midX), Int(f.midY), Int(f.width), Int(f.height))
case "next":
    let hits = all.indices.filter { matches(all[$0], args[2]) }
    let n = args.count > 4 ? Int(args[4])! : 1
    guard hits.count >= n,
          let i = all[(hits[n - 1] + 1)...].firstIndex(where: { $0.role == args[3] })
    else { FileHandle.standardError.write("no \(args[3]) after \(args[2])\n".data(using: .utf8)!); exit(1) }
    let f = all[i].frame
    print(Int(f.midX), Int(f.midY), Int(f.width), Int(f.height))
case "slider":
    let label = args[2]
    let hits = all.indices.filter { all[$0].role == "AXStaticText" && all[$0].value == label }
    // Counted from 1; the default, -1, is the last: the inspector's, which
    // comes after the cards.
    let n = args.count > 3 ? Int(args[3])! : -1
    let k = n < 0 ? hits.count + n : n - 1
    guard k >= 0, k < hits.count,
          let ti = all[(hits[k] + 1)...].firstIndex(where: { $0.role == "AXGroup" && $0.frame.height <= 6 }),
          let si = all[(hits[k] + 1)...].firstIndex(where: { $0.role == "AXSlider" })
    else { FileHandle.standardError.write("no slider after \(label)\n".data(using: .utf8)!); exit(1) }
    let tr = all[ti].frame, th = all[si].frame
    print(Int(th.midX), Int(tr.midY), Int(tr.minX), Int(tr.width))
default:
    let q = args.count > 2 ? args[2] : ""
    for f in all where q.isEmpty || matches(f, q) || f.role.localizedCaseInsensitiveContains(q) {
        print("\(f.role)\t\(f.name)\t\(f.value.prefix(40))\t\(Int(f.frame.minX)),\(Int(f.frame.minY)) \(Int(f.frame.width))x\(Int(f.frame.height))")
    }
}

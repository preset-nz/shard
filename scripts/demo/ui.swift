// ui move x y | click x y | dclick x y | drag x1 y1 x2 y2 [ms] | type "text" | key <code> [cmd|shift|opt...]
// | hold <code> <ms> (a key held down, as Brake wants) | pos
import Foundation
import CoreGraphics

let a = CommandLine.arguments
let src = CGEventSource(stateID: .hidSystemState)
func post(_ e: CGEvent?) { e?.post(tap: .cghidEventTap) }
func mouse(_ t: CGEventType, _ p: CGPoint, clicks: Int64 = 1) {
    let e = CGEvent(mouseEventSource: src, mouseType: t, mouseCursorPosition: p, mouseButton: .left)
    e?.setIntegerValueField(.mouseEventClickState, value: clicks)
    post(e)
}
func glide(from: CGPoint, to: CGPoint, ms: Double, type: CGEventType) {
    let steps = max(2, Int(ms / 8))
    for i in 1...steps {
        let f = Double(i) / Double(steps)
        let s = f * f * (3 - 2 * f)
        mouse(type, CGPoint(x: from.x + (to.x - from.x) * s, y: from.y + (to.y - from.y) * s))
        usleep(useconds_t(ms * 1000 / Double(steps)))
    }
}
func here() -> CGPoint { CGEvent(source: nil)?.location ?? .zero }
func pt(_ i: Int) -> CGPoint { CGPoint(x: Double(a[i])!, y: Double(a[i + 1])!) }
switch a[1] {
case "pos": let p = here(); print(Int(p.x), Int(p.y))
case "move": glide(from: here(), to: pt(2), ms: 400, type: .mouseMoved)
case "click", "dclick":
    let p = pt(2); glide(from: here(), to: p, ms: 400, type: .mouseMoved)
    let n: Int64 = a[1] == "dclick" ? 2 : 1
    for c in 1...n { mouse(.leftMouseDown, p, clicks: c); usleep(30000); mouse(.leftMouseUp, p, clicks: c); usleep(60000) }
case "drag":
    let p = pt(2), q = pt(4); let ms = a.count > 6 ? Double(a[6])! : 900
    glide(from: here(), to: p, ms: 400, type: .mouseMoved)
    mouse(.leftMouseDown, p); usleep(80000)
    glide(from: p, to: q, ms: ms, type: .leftMouseDragged)
    usleep(80000); mouse(.leftMouseUp, q)
case "type":
    for ch in a[2].utf16 {
        var c = ch
        let d = CGEvent(keyboardEventSource: src, virtualKey: 0, keyDown: true)
        d?.keyboardSetUnicodeString(stringLength: 1, unicodeString: &c); post(d)
        let u = CGEvent(keyboardEventSource: src, virtualKey: 0, keyDown: false)
        u?.keyboardSetUnicodeString(stringLength: 1, unicodeString: &c); post(u)
        usleep(45000)
    }
case "key":
    var flags: CGEventFlags = []
    for m in a.dropFirst(3) {
        switch m { case "cmd": flags.insert(.maskCommand); case "shift": flags.insert(.maskShift)
        case "opt": flags.insert(.maskAlternate); case "ctrl": flags.insert(.maskControl); default: break }
    }
    let code = CGKeyCode(a[2])!
    let d = CGEvent(keyboardEventSource: src, virtualKey: code, keyDown: true); d?.flags = flags; post(d)
    let u = CGEvent(keyboardEventSource: src, virtualKey: code, keyDown: false); u?.flags = flags; post(u)
case "hold":
    let code = CGKeyCode(a[2])!
    post(CGEvent(keyboardEventSource: src, virtualKey: code, keyDown: true))
    usleep(useconds_t(Double(a[3])! * 1000))
    post(CGEvent(keyboardEventSource: src, virtualKey: code, keyDown: false))
default: print("unknown")
}

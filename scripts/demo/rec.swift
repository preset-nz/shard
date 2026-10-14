// rec <out.mov> <seconds>: records the main display with system audio.
import Foundation
import ScreenCaptureKit
import AVFoundation
import AppKit

let args = CommandLine.arguments
let out = URL(fileURLWithPath: args[1])
let seconds = Double(args[2]) ?? 60

final class Delegate: NSObject, SCRecordingOutputDelegate, SCStreamDelegate {
    func recordingOutputDidStartRecording(_ r: SCRecordingOutput) { print("recording"); fflush(stdout) }
    func recordingOutput(_ r: SCRecordingOutput, didFailWithError e: Error) { print("failed: \(e)"); exit(1) }
    func recordingOutputDidFinishRecording(_ r: SCRecordingOutput) { print("finished \(out.path)"); exit(0) }
    func stream(_ s: SCStream, didStopWithError e: Error) { print("stream stopped: \(e)"); exit(1) }
}
let delegate = Delegate()

DispatchQueue.main.async { Task { @MainActor in
    do {
        let content = try await SCShareableContent.excludingDesktopWindows(false, onScreenWindowsOnly: true)
        guard let display = content.displays.first else { print("no display"); exit(1) }
        let filter = SCContentFilter(display: display, excludingWindows: [])
        let cfg = SCStreamConfiguration()
        cfg.width = display.width * 2
        cfg.height = display.height * 2
        cfg.minimumFrameInterval = CMTime(value: 1, timescale: 30)
        cfg.capturesAudio = true
        cfg.sampleRate = 48000
        cfg.channelCount = 2
        cfg.showsCursor = true
        let stream = SCStream(filter: filter, configuration: cfg, delegate: delegate)
        let rc = SCRecordingOutputConfiguration()
        rc.outputURL = out
        rc.outputFileType = .mov
        rc.videoCodecType = .hevc
        let rec = SCRecordingOutput(configuration: rc, delegate: delegate)
        try stream.addRecordingOutput(rec)
        try await stream.startCapture()
        let stop = { Task { try? await stream.stopCapture(); try? stream.removeRecordingOutput(rec) } }
        signal(SIGINT, SIG_IGN)
        let src = DispatchSource.makeSignalSource(signal: SIGINT, queue: .main)
        src.setEventHandler { _ = stop() }
        src.resume()
        DispatchQueue.main.asyncAfter(deadline: .now() + seconds) { _ = stop() }
        withExtendedLifetime(src) {}
        globalSrc = src
    } catch { print("error: \(error)"); exit(1) }
} }
nonisolated(unsafe) var globalSrc: DispatchSourceSignal?
let app = NSApplication.shared
app.setActivationPolicy(.accessory)
app.run()

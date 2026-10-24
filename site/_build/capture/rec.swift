import Foundation
import ScreenCaptureKit
import AVFoundation
import CoreMedia
import AppKit
_ = NSApplication.shared

// usage: rec <windowID> <seconds> <out.mov>
let wid = CGWindowID(UInt32(CommandLine.arguments[1])!)
let secs = Double(CommandLine.arguments[2])!
let out = URL(fileURLWithPath: CommandLine.arguments[3])
try? FileManager.default.removeItem(at: out)

final class Rec: NSObject, SCStreamOutput {
  let writer: AVAssetWriter; let input: AVAssetWriterInput; var started = false
  init(url: URL, w: Int, h: Int) throws {
    writer = try AVAssetWriter(outputURL: url, fileType: .mov)
    input = AVAssetWriterInput(mediaType: .video, outputSettings: [AVVideoCodecKey: AVVideoCodecType.h264, AVVideoWidthKey: w, AVVideoHeightKey: h,
      AVVideoCompressionPropertiesKey: [AVVideoAverageBitRateKey: 12_000_000]])
    input.expectsMediaDataInRealTime = true; writer.add(input)
  }
  func stream(_ s: SCStream, didOutputSampleBuffer sb: CMSampleBuffer, of type: SCStreamOutputType) {
    guard type == .screen, sb.isValid,
      let att = (CMSampleBufferGetSampleAttachmentsArray(sb, createIfNecessary: false) as? [[SCStreamFrameInfo: Any]])?.first,
      let raw = att[.status] as? Int, SCFrameStatus(rawValue: raw) == .complete else { return }
    if !started { writer.startWriting(); writer.startSession(atSourceTime: sb.presentationTimeStamp); started = true }
    if input.isReadyForMoreMediaData { input.append(sb) }
  }
}

Task {
  let content = try await SCShareableContent.excludingDesktopWindows(false, onScreenWindowsOnly: false)
  guard let win = content.windows.first(where: { $0.windowID == wid }) else { print("no window"); exit(1) }
  let filter = SCContentFilter(desktopIndependentWindow: win)
  let cfg = SCStreamConfiguration()
  let scale = 2.0
  cfg.width = Int(win.frame.width * scale); cfg.height = Int(win.frame.height * scale)
  cfg.minimumFrameInterval = CMTime(value: 1, timescale: 30); cfg.showsCursor = false
  cfg.ignoreShadowsSingleWindow = true
  let rec = try Rec(url: out, w: cfg.width, h: cfg.height)
  let stream = SCStream(filter: filter, configuration: cfg, delegate: nil)
  try stream.addStreamOutput(rec, type: .screen, sampleHandlerQueue: DispatchQueue(label: "rec"))
  try await stream.startCapture()
  print("recording"); fflush(stdout)
  try await Task.sleep(nanoseconds: UInt64(secs * 1e9))
  try await stream.stopCapture()
  rec.input.markAsFinished(); await rec.writer.finishWriting()
  print("saved", out.path); exit(0)
}
RunLoop.main.run()

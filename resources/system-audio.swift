import Foundation
import ScreenCaptureKit
import CoreMedia
import AudioToolbox

// Runs inside the signed app, so macOS attributes capture permission to SpeakMore.
public typealias AudioCallback = @convention(c) (UnsafeMutableRawPointer?, UnsafePointer<Float>?, Int32, Double) -> Void
final class AudioCapture: NSObject, SCStreamOutput, SCStreamDelegate {
    let callback: AudioCallback
    let context: UnsafeMutableRawPointer?
    let queue = DispatchQueue(label: "com.speakmore.system-audio")
    var stream: SCStream?
    // Accessed only on the serial callback queue. No callback may outlive its Rust context.
    var closed = false
    init(_ callback: @escaping AudioCallback, _ context: UnsafeMutableRawPointer?) { self.callback = callback; self.context = context }
    func stream(_ stream: SCStream, didStopWithError error: Error) {
        queue.async { [weak self] in
            guard let self, !self.closed else { return }
            self.callback(self.context, nil, -1, 0)
        }
    }
    func stream(_ stream: SCStream, didOutputSampleBuffer sample: CMSampleBuffer, of type: SCStreamOutputType) {
        guard !closed, type == .audio, sample.isValid, let desc = sample.formatDescription,
              let asbd = CMAudioFormatDescriptionGetStreamBasicDescription(desc), asbd.pointee.mBitsPerChannel == 32,
              (asbd.pointee.mFormatFlags & kAudioFormatFlagIsFloat) != 0 else { return }
        var retained: CMBlockBuffer?
        var list = AudioBufferList(mNumberBuffers: 1, mBuffers: AudioBuffer(mNumberChannels: 1, mDataByteSize: 0, mData: nil))
        let status = CMSampleBufferGetAudioBufferListWithRetainedBlockBuffer(sample, bufferListSizeNeededOut: nil, bufferListOut: &list, bufferListSize: MemoryLayout<AudioBufferList>.size, blockBufferAllocator: kCFAllocatorDefault, blockBufferMemoryAllocator: kCFAllocatorDefault, flags: 0, blockBufferOut: &retained)
        guard status == noErr, list.mNumberBuffers == 1, list.mBuffers.mNumberChannels == 1, let data = list.mBuffers.mData else { callback(context, nil, -2, 0); return }
        guard list.mBuffers.mDataByteSize > 0 else { return }
        callback(context, data.assumingMemoryBound(to: Float.self), Int32(list.mBuffers.mDataByteSize / 4), CMSampleBufferGetPresentationTimeStamp(sample).seconds)
    }
}
@_cdecl("sm_audio_start")
public func start(_ pid: Int32, _ callback: @escaping AudioCallback, _ context: UnsafeMutableRawPointer?) -> UnsafeMutableRawPointer? {
    let capture = AudioCapture(callback, context)
    let done = DispatchSemaphore(value: 0)
    var success = false
    Task {
        defer { done.signal() }
        do {
            let content = try await SCShareableContent.excludingDesktopWindows(false, onScreenWindowsOnly: false)
            guard let display = content.displays.first else { return }
            let filter: SCContentFilter
            if pid > 0 {
                guard let app = content.applications.first(where: { $0.processID == pid }) else { return }
                filter = SCContentFilter(display: display, including: [app], exceptingWindows: [])
            } else { filter = SCContentFilter(display: display, excludingApplications: content.applications.filter { $0.processID == ProcessInfo.processInfo.processIdentifier }, exceptingWindows: []) }
            let config = SCStreamConfiguration()
            config.capturesAudio = true; config.excludesCurrentProcessAudio = true
            config.sampleRate = 16000; config.channelCount = 1
            config.width = 2; config.height = 2; config.minimumFrameInterval = CMTime(value: 1, timescale: 1)
            let stream = SCStream(filter: filter, configuration: config, delegate: capture)
            try stream.addStreamOutput(capture, type: .audio, sampleHandlerQueue: capture.queue)
            capture.stream = stream
            try await stream.startCapture()
            success = true
        } catch {
            try? await capture.stream?.stopCapture()
            capture.queue.sync { capture.closed = true }
            capture.stream = nil
            success = false
        }
    }
    done.wait() // Called exclusively on a background thread, never the AppKit thread.
    return success ? Unmanaged.passRetained(capture).toOpaque() : nil
}
@_cdecl("sm_audio_stop")
public func stop(_ raw: UnsafeMutableRawPointer) {
    let capture = Unmanaged<AudioCapture>.fromOpaque(raw).takeRetainedValue()
    let done = DispatchSemaphore(value: 0)
    let stream = capture.stream
    Task { try? await stream?.stopCapture(); done.signal() }
    // A stuck stopCapture must not hang the caller; closing on the callback queue still fences every callback.
    _ = done.wait(timeout: .now() + .seconds(3)); capture.queue.sync { capture.closed = true }; capture.stream = nil
}
@_cdecl("sm_audio_apps")
public func apps() -> UnsafeMutablePointer<CChar>? {
    let done = DispatchSemaphore(value: 0)
    var result = "{\"error\":\"AudioAppsUnavailable\"}"
    Task {
        defer { done.signal() }
        do {
            let c = try await SCShareableContent.excludingDesktopWindows(false, onScreenWindowsOnly: false)
            let values = c.applications.filter { !$0.applicationName.isEmpty && $0.processID != ProcessInfo.processInfo.processIdentifier }
                .sorted { $0.applicationName.localizedStandardCompare($1.applicationName) == .orderedAscending }
                .map { ["pid": $0.processID, "name": $0.applicationName, "bundleId": $0.bundleIdentifier] as [String: Any] }
            let data = try JSONSerialization.data(withJSONObject: ["apps": values])
            if let s = String(data: data, encoding: .utf8) { result = s }
        } catch {
            let native = error as NSError
            // SCStreamErrorUserDeclined from SCError.h. Do not turn a denied request into an empty list.
            if native.domain == SCStreamErrorDomain && native.code == -3801 {
                result = "{\"error\":\"SystemAudioPermission\"}"
            }
        }
    }
    done.wait(); return strdup(result)
}
@_cdecl("sm_audio_free")
public func freeString(_ p: UnsafeMutableRawPointer) { free(p) }

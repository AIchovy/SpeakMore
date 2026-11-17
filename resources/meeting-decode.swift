import Foundation
import AVFoundation

// No microphone or network access. Paths come only from the native file picker.
do {
    let args = CommandLine.arguments
    guard args.count >= 3 else { throw NSError(domain: "arguments", code: 1) }
    let file = try AVAudioFile(forReading: URL(fileURLWithPath: args[2]))
    let format = file.processingFormat
    let duration = Double(file.length) / format.sampleRate
    guard duration.isFinite, duration > 0, format.channelCount <= 8 else { throw NSError(domain: "format", code: 1) }
    if args[1] == "probe" {
        let value: [String: Any] = ["duration": duration]
        FileHandle.standardOutput.write(try JSONSerialization.data(withJSONObject: value))
    } else {
        guard args.count == 6, let start = Double(args[4]), let length = Double(args[5]), start >= 0, length > 0, length <= 3600, start < duration else { throw NSError(domain: "range", code: 1) }
        let outputURL = URL(fileURLWithPath: args[3])
        guard FileManager.default.createFile(atPath: outputURL.path, contents: nil, attributes: [.posixPermissions: 0o600]) else {throw NSError(domain: "storage", code: 1)}
        let output = try FileHandle(forWritingTo: outputURL)
        defer { try? output.close() }
        file.framePosition = AVAudioFramePosition(start * format.sampleRate)
        var remaining = min(file.length-file.framePosition, AVAudioFramePosition(length * format.sampleRate))
        let target = AVAudioFormat(commonFormat: .pcmFormatFloat32, sampleRate: 16000, channels: 1, interleaved: false)!
        guard let converter = AVAudioConverter(from: format, to: target) else { throw NSError(domain: "converter", code: 1) }
        let buffer = AVAudioPCMBuffer(pcmFormat: target, frameCapacity: 4096)!
        var readError: Error?
        var written = 0
        let maxSamples = Int(min(duration-start, length) * 16000)
        while written < maxSamples {
            var error: NSError?
            let status = converter.convert(to: buffer, error: &error) { count, state in
                if remaining == 0 { state.pointee = .endOfStream; return nil }
                let n = min(AVAudioFrameCount(remaining), count)
                let input = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: n)!
                do { try file.read(into: input, frameCount: n) }
                catch { readError = error; state.pointee = .endOfStream; return nil }
                remaining -= AVAudioFramePosition(input.frameLength)
                state.pointee = input.frameLength == 0 ? .endOfStream : .haveData
                if input.frameLength == 0 { remaining = 0 }
                return input
            }
            if let error = readError ?? error {throw error}
            let n = min(Int(buffer.frameLength), maxSamples-written)
            if n > 0 {
                let samples = buffer.floatChannelData![0]
                guard (0..<n).allSatisfy({samples[$0].isFinite}) else {throw NSError(domain: "samples", code: 1)}
                try output.write(contentsOf: Data(bytes: samples, count: n*4))
                written += n
            }
            if status == .endOfStream {break}
            if status == .error || n == 0 { throw NSError(domain: "decode", code: 1) }
        }
        try output.synchronize()
        guard written >= 4000 else { throw NSError(domain: "short", code: 1) }
    }
} catch {
    FileHandle.standardError.write(Data("Audio decode failed\n".utf8))
    exit(1)
}

import CoreMedia
import Foundation

/// Reassembles Annex-B H.264 NAL units (start-code-delimited: `00 00 00 01`
/// or `00 00 01`) from a raw byte stream — what FFmpeg's `-f h264` output
/// produces — and turns them into `CMFormatDescription`/`CMSampleBuffer`
/// for `VideoDecoder`.
///
/// Assumes in-order, lossless delivery. UDP guarantees neither — a dropped
/// or reordered datagram corrupts NAL parsing until the next SPS/PPS+IDR
/// sequence recovers it (there's only ever one, sent once, at the very
/// start of a stream — see `encode.rs`'s "known gap" doc comment on the
/// Windows side for the matching caveat). A real implementation would need
/// sequence numbers, periodic keyframes with repeated parameter sets, or a
/// more resilient transport (SRT, which the encoder side already has a
/// documented path to) to do better. Not attempted here — first pass is
/// "prove the parsing and decode pipeline is correct at all," same scope
/// discipline as everything else in this module.
final class NALUnitParser {
    private var buffer = Data()
    private var spsData: Data?
    private var ppsData: Data?
    private var frameCounter: Int64 = 0

    var onFormatDescription: ((CMFormatDescription) -> Void)?
    var onSampleBuffer: ((CMSampleBuffer) -> Void)?

    func feed(_ data: Data) {
        buffer.append(data)
        extractCompleteNALUnits()
    }

    private func extractCompleteNALUnits() {
        let starts = findStartCodeRanges(in: buffer)
        guard starts.count > 1 else { return } // need 2 start codes to bound 1 complete NAL

        for i in 0..<(starts.count - 1) {
            let nalStart = starts[i].upperBound
            let nalEnd = starts[i + 1].lowerBound
            guard nalStart < nalEnd else { continue }
            handle(nal: buffer.subdata(in: nalStart..<nalEnd))
        }

        // Keep from the last start code onward — that NAL isn't known to be
        // complete yet (more data for it may still be in flight).
        if let lastStart = starts.last {
            buffer.removeSubrange(buffer.startIndex..<lastStart.lowerBound)
        }
    }

    private func handle(nal: Data) {
        guard let first = nal.first else { return }
        let nalType = first & 0x1F

        switch nalType {
        case 7: // SPS
            spsData = nal
            tryBuildFormatDescription()
        case 8: // PPS
            ppsData = nal
            tryBuildFormatDescription()
        case 1, 5: // non-IDR / IDR slice — the actual frame data
            guard formatDescriptionForSamples != nil else {
                // Arrived before SPS/PPS — a dropped/reordered leading
                // packet, most likely. Nothing decodable without a format
                // description; drop it rather than crash on the force
                // unwrap in emitSampleBuffer.
                return
            }
            emitSampleBuffer(nal: nal)
        default:
            break // SEI, AUD, etc. — not needed for decode, ignored
        }
    }

    private func tryBuildFormatDescription() {
        guard let sps = spsData, let pps = ppsData else { return }

        var formatDescription: CMFormatDescription?
        let status: OSStatus = sps.withUnsafeBytes { spsBytes in
            pps.withUnsafeBytes { ppsBytes in
                let spsPointer = spsBytes.bindMemory(to: UInt8.self).baseAddress!
                let ppsPointer = ppsBytes.bindMemory(to: UInt8.self).baseAddress!
                let pointers: [UnsafePointer<UInt8>] = [spsPointer, ppsPointer]
                let sizes: [Int] = [sps.count, pps.count]

                return pointers.withUnsafeBufferPointer { pointersBuffer in
                    sizes.withUnsafeBufferPointer { sizesBuffer in
                        CMVideoFormatDescriptionCreateFromH264ParameterSets(
                            allocator: kCFAllocatorDefault,
                            parameterSetCount: 2,
                            parameterSetPointers: pointersBuffer.baseAddress!,
                            parameterSetSizes: sizesBuffer.baseAddress!,
                            nalUnitHeaderLength: 4,
                            formatDescriptionOut: &formatDescription
                        )
                    }
                }
            }
        }

        guard status == noErr, let formatDescription else {
            print("NALUnitParser: failed to build format description, OSStatus \(status)")
            return
        }
        formatDescriptionForSamples = formatDescription
        onFormatDescription?(formatDescription)
    }

    private func emitSampleBuffer(nal: Data) {
        // VideoToolbox wants AVCC framing (4-byte big-endian length prefix)
        // for CMSampleBuffers, not the Annex-B start code this was parsed
        // out of — matches the `nalUnitHeaderLength: 4` used above.
        var lengthPrefixed = Data()
        var nalLength = UInt32(nal.count).bigEndian
        withUnsafeBytes(of: &nalLength) { lengthPrefixed.append(contentsOf: $0) }
        lengthPrefixed.append(nal)

        var blockBuffer: CMBlockBuffer?
        let blockStatus = CMBlockBufferCreateWithMemoryBlock(
            allocator: kCFAllocatorDefault,
            memoryBlock: nil, // let CMBlockBuffer allocate/own its own storage
            blockLength: lengthPrefixed.count,
            blockAllocator: kCFAllocatorDefault,
            customBlockSource: nil,
            offsetToData: 0,
            dataLength: lengthPrefixed.count,
            flags: 0,
            blockBufferOut: &blockBuffer
        )
        guard blockStatus == kCMBlockBufferNoErr, let blockBuffer else { return }

        let copyStatus = lengthPrefixed.withUnsafeBytes { rawBuffer -> OSStatus in
            CMBlockBufferReplaceDataBytes(
                with: rawBuffer.baseAddress!,
                blockBuffer: blockBuffer,
                offsetIntoDestination: 0,
                dataLength: lengthPrefixed.count
            )
        }
        guard copyStatus == kCMBlockBufferNoErr else { return }

        // No real timestamps available over this simple transport (no RTP,
        // no PTS in the raw H.264 byte stream) — a monotonic counter is
        // fine since the encoder runs zero B-frames (decode order ==
        // presentation order), so there's no reordering for real
        // timestamps to resolve anyway.
        frameCounter += 1
        var timingInfo = CMSampleTimingInfo(
            duration: .invalid,
            presentationTimeStamp: CMTime(value: frameCounter, timescale: 30),
            decodeTimeStamp: .invalid
        )

        var sampleSize = lengthPrefixed.count
        var sampleBuffer: CMSampleBuffer?
        let sampleStatus = CMSampleBufferCreate(
            allocator: kCFAllocatorDefault,
            dataBuffer: blockBuffer,
            dataReady: true,
            makeDataReadyCallback: nil,
            refcon: nil,
            formatDescription: formatDescriptionForSamples,
            sampleCount: 1,
            sampleTimingEntryCount: 1,
            sampleTimingArray: &timingInfo,
            sampleSizeEntryCount: 1,
            sampleSizeArray: &sampleSize,
            sampleBufferOut: &sampleBuffer
        )

        guard sampleStatus == noErr, let sampleBuffer else {
            print("NALUnitParser: CMSampleBufferCreate failed, OSStatus \(sampleStatus)")
            return
        }
        onSampleBuffer?(sampleBuffer)
    }

    /// Set once `onFormatDescription` fires — `emitSampleBuffer` needs it
    /// for every sample buffer, not just the first.
    var formatDescriptionForSamples: CMFormatDescription!

    private func findStartCodeRanges(in data: Data) -> [Range<Data.Index>] {
        var ranges: [Range<Data.Index>] = []
        var i = data.startIndex
        let end = data.endIndex

        while i < end {
            if i + 4 <= end, data[i] == 0, data[i + 1] == 0, data[i + 2] == 0, data[i + 3] == 1 {
                ranges.append(i..<(i + 4))
                i += 4
                continue
            }
            if i + 3 <= end, data[i] == 0, data[i + 1] == 0, data[i + 2] == 1 {
                ranges.append(i..<(i + 3))
                i += 3
                continue
            }
            i += 1
        }
        return ranges
    }
}

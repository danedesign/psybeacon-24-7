import CoreMedia
import VideoToolbox

/// Wraps `VTDecompressionSession` for low-latency H.264 hardware decode.
///
/// This is untested against a compiler — unlike everything on the Windows
/// side, there's no way to verify VideoToolbox API details (exact parameter
/// labels, callback signature) from this environment. Written from
/// documented API shape; expect the first `swift build` to need minor
/// corrections, the same way early `windows-rs` code needed `cargo check`
/// to nail down exact signatures.
final class VideoDecoder {
    private var session: VTDecompressionSession?

    /// Called on VideoToolbox's own callback thread, not necessarily the
    /// thread `decode` was called from — hop to the main thread before
    /// touching UI/AppKit from inside this closure.
    var onDecodedFrame: ((CVPixelBuffer) -> Void)?

    enum DecoderError: Error {
        case sessionCreationFailed(OSStatus)
    }

    /// Must be called once, with the format description from the stream's
    /// first sample (carries SPS/PPS), before any `decode` call.
    func configure(formatDescription: CMFormatDescription) throws {
        var callback = VTDecompressionOutputCallbackRecord(
            decompressionOutputCallback: decompressionOutputCallback,
            decompressionOutputRefCon: Unmanaged.passUnretained(self).toOpaque()
        )

        let decoderSpecification: [CFString: Any] = [
            kVTVideoDecoderSpecification_EnableHardwareAcceleratedVideoDecoder: true
        ]

        // NV12 (biplanar 4:2:0) + Metal-compatible: MetalRenderer samples
        // the Y and CbCr planes directly as separate textures rather than
        // paying for a CPU-side RGB conversion.
        let destinationImageBufferAttributes: [CFString: Any] = [
            kCVPixelBufferPixelFormatTypeKey: kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange,
            kCVPixelBufferMetalCompatibilityKey: true,
        ]

        var newSession: VTDecompressionSession?
        let status = VTDecompressionSessionCreate(
            allocator: kCFAllocatorDefault,
            formatDescription: formatDescription,
            decoderSpecification: decoderSpecification as CFDictionary,
            imageBufferAttributes: destinationImageBufferAttributes as CFDictionary,
            outputCallback: &callback,
            decompressionSessionOut: &newSession
        )

        guard status == noErr, let newSession else {
            throw DecoderError.sessionCreationFailed(status)
        }
        session = newSession
    }

    func decode(sampleBuffer: CMSampleBuffer) {
        guard let session else { return }
        var flagsOut = VTDecodeInfoFlags()
        VTDecompressionSessionDecodeFrame(
            session,
            sampleBuffer: sampleBuffer,
            flags: [],
            frameRefcon: nil,
            infoFlagsOut: &flagsOut
        )
    }
}

private func decompressionOutputCallback(
    decompressionOutputRefCon: UnsafeMutableRawPointer?,
    sourceFrameRefCon: UnsafeMutableRawPointer?,
    status: OSStatus,
    infoFlags: VTDecodeInfoFlags,
    imageBuffer: CVImageBuffer?,
    presentationTimeStamp: CMTime,
    presentationDuration: CMTime
) {
    guard status == noErr, let imageBuffer, let refCon = decompressionOutputRefCon else {
        if status != noErr {
            print("VideoDecoder: decode callback got status \(status)")
        }
        return
    }
    let decoder = Unmanaged<VideoDecoder>.fromOpaque(refCon).takeUnretainedValue()
    decoder.onDecodedFrame?(imageBuffer)
}

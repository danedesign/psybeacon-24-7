import CoreVideo
import MetalKit

/// Renders decoded video frames (NV12 `CVPixelBuffer`s from `VideoDecoder`)
/// via Metal. Samples the Y and CbCr planes as separate textures straight
/// from the `CVPixelBuffer` through `CVMetalTextureCache` — no CPU-side
/// copy or colorspace conversion; the YCbCr→RGB conversion happens in the
/// fragment shader below, on the GPU, same as the plane sampling.
///
/// Untested — see `VideoDecoder`'s doc comment; the same caveat applies to
/// every CoreVideo/Metal API call here.
final class MetalRenderer: NSObject, MTKViewDelegate {
    enum RendererError: Error {
        case commandQueueCreationFailed
        case textureCacheCreationFailed
    }

    private let device: MTLDevice
    private let commandQueue: MTLCommandQueue
    private let textureCache: CVMetalTextureCache
    private let pipelineState: MTLRenderPipelineState

    private let lock = NSLock()
    private var currentPixelBuffer: CVPixelBuffer?

    init(device: MTLDevice) throws {
        self.device = device

        guard let queue = device.makeCommandQueue() else {
            throw RendererError.commandQueueCreationFailed
        }
        commandQueue = queue

        var cache: CVMetalTextureCache?
        CVMetalTextureCacheCreate(kCFAllocatorDefault, nil, device, nil, &cache)
        guard let cache else { throw RendererError.textureCacheCreationFailed }
        textureCache = cache

        pipelineState = try Self.makePipelineState(device: device)
        super.init()
    }

    /// Call from any thread (this is what `VideoDecoder.onDecodedFrame`
    /// calls into) — just swaps a reference under a lock. Actual drawing
    /// happens later, on `draw(in:)`, driven by `MTKView`'s own display
    /// link.
    func present(_ pixelBuffer: CVPixelBuffer) {
        lock.lock()
        currentPixelBuffer = pixelBuffer
        lock.unlock()
    }

    func mtkView(_ view: MTKView, drawableSizeWillChange size: CGSize) {}

    func draw(in view: MTKView) {
        lock.lock()
        let pixelBuffer = currentPixelBuffer
        lock.unlock()

        guard let pixelBuffer,
            let drawable = view.currentDrawable,
            let renderPassDescriptor = view.currentRenderPassDescriptor
        else { return }

        let width = CVPixelBufferGetWidthOfPlane(pixelBuffer, 0)
        let height = CVPixelBufferGetHeightOfPlane(pixelBuffer, 0)
        let cbcrWidth = CVPixelBufferGetWidthOfPlane(pixelBuffer, 1)
        let cbcrHeight = CVPixelBufferGetHeightOfPlane(pixelBuffer, 1)

        var yTextureRef: CVMetalTexture?
        CVMetalTextureCacheCreateTextureFromImage(
            kCFAllocatorDefault, textureCache, pixelBuffer, nil,
            .r8Unorm, width, height, 0, &yTextureRef
        )
        var cbcrTextureRef: CVMetalTexture?
        CVMetalTextureCacheCreateTextureFromImage(
            kCFAllocatorDefault, textureCache, pixelBuffer, nil,
            .rg8Unorm, cbcrWidth, cbcrHeight, 1, &cbcrTextureRef
        )

        guard let yTextureRef, let cbcrTextureRef,
            let yTexture = CVMetalTextureGetTexture(yTextureRef),
            let cbcrTexture = CVMetalTextureGetTexture(cbcrTextureRef)
        else { return }

        guard let commandBuffer = commandQueue.makeCommandBuffer(),
            let encoder = commandBuffer.makeRenderCommandEncoder(descriptor: renderPassDescriptor)
        else { return }

        encoder.setRenderPipelineState(pipelineState)
        encoder.setFragmentTexture(yTexture, index: 0)
        encoder.setFragmentTexture(cbcrTexture, index: 1)
        encoder.drawPrimitives(type: .triangleStrip, vertexStart: 0, vertexCount: 4)
        encoder.endEncoding()

        commandBuffer.present(drawable)
        commandBuffer.commit()

        // Stale CVMetalTextureCache entries pin GPU memory until flushed;
        // harmless to skip for a short test, but matters once this runs
        // continuously for a real streaming session.
        CVMetalTextureCacheFlush(textureCache, 0)
    }

    private static func makePipelineState(device: MTLDevice) throws -> MTLRenderPipelineState {
        let library = try device.makeLibrary(source: shaderSource, options: nil)
        let descriptor = MTLRenderPipelineDescriptor()
        descriptor.vertexFunction = library.makeFunction(name: "vertexPassthrough")
        descriptor.fragmentFunction = library.makeFunction(name: "fragmentYCbCrToRGB")
        descriptor.colorAttachments[0].pixelFormat = .bgra8Unorm
        return try device.makeRenderPipelineState(descriptor: descriptor)
    }

    private static let shaderSource = """
        #include <metal_stdlib>
        using namespace metal;

        struct VertexOut {
            float4 position [[position]];
            float2 texCoord;
        };

        vertex VertexOut vertexPassthrough(uint vertexID [[vertex_id]]) {
            const float2 positions[4] = { float2(-1,-1), float2(1,-1), float2(-1,1), float2(1,1) };
            const float2 texCoords[4] = { float2(0,1), float2(1,1), float2(0,0), float2(1,0) };
            VertexOut out;
            out.position = float4(positions[vertexID], 0, 1);
            out.texCoord = texCoords[vertexID];
            return out;
        }

        fragment float4 fragmentYCbCrToRGB(VertexOut in [[stage_in]],
                                            texture2d<float> yTexture [[texture(0)]],
                                            texture2d<float> cbcrTexture [[texture(1)]]) {
            constexpr sampler s(filter::linear);
            float y = yTexture.sample(s, in.texCoord).r;
            float2 cbcr = cbcrTexture.sample(s, in.texCoord).rg - float2(0.5, 0.5);
            float3 rgb;
            rgb.r = y + 1.402 * cbcr.y;
            rgb.g = y - 0.344 * cbcr.x - 0.714 * cbcr.y;
            rgb.b = y + 1.772 * cbcr.x;
            return float4(rgb, 1.0);
        }
        """
}

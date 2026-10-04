package dev.horizonlab.dispatch.fixture;

import com.mojang.renderpearl.api.buffers.GpuBuffer;
import com.mojang.renderpearl.api.buffers.GpuBufferSlice;
import com.mojang.renderpearl.api.commands.CommandEncoder;
import com.mojang.renderpearl.api.device.GpuDevice;
import com.llamalad7.mixinextras.injector.wrapmethod.WrapMethod;
import com.llamalad7.mixinextras.injector.wrapoperation.Operation;
import com.seibel.distanthorizons.common.render.blaze.wrappers.buffer.BlazeVertexBufferWrapper;
import dev.horizonlab.metrics.MacBenchmarkIntegration;
import java.util.function.Supplier;
import java.nio.ByteBuffer;
import org.spongepowered.asm.mixin.Mixin;
import org.spongepowered.asm.mixin.Shadow;
import org.spongepowered.asm.mixin.injection.At;
import org.spongepowered.asm.mixin.injection.Redirect;

/** Private allocation timings; the original allocation and upload remain intact. */
@Mixin(BlazeVertexBufferWrapper.class)
public abstract class MacDhVertexAllocationMixin {
    @Shadow public GpuBuffer vertexGpuBuffer;

    @WrapMethod(method = "uploadVertexBuffer")
    private void horizon$upload(ByteBuffer data, int vertices, Operation<Void> original) {
        long bytes = data.remaining();
        long started = System.nanoTime();
        var event = MacBenchmarkIntegration.beginTerrainUpload(bytes);
        try { original.call(data, vertices); }
        finally {
            MacBenchmarkIntegration.profileVboUpload(bytes, started, System.nanoTime() - started, false);
            if (event != null) { event.end(); event.commit(); }
        }
    }

    @Redirect(method = "uploadVertexBuffer", at = @At(value = "INVOKE", target =
        "Lcom/mojang/renderpearl/api/commands/CommandEncoder;writeToBuffer(Lcom/mojang/renderpearl/api/buffers/GpuBufferSlice;Ljava/nio/ByteBuffer;)V"))
    private void horizon$copy(CommandEncoder encoder, GpuBufferSlice destination, ByteBuffer data) {
        long bytes = data.remaining();
        long started = System.nanoTime();
        try { encoder.writeToBuffer(destination, data); }
        finally { MacBenchmarkIntegration.profileVboUpload(bytes, started, System.nanoTime() - started, true); }
    }

    @Redirect(method = "uploadVertexBuffer", at = @At(value = "INVOKE", target =
        "Lcom/mojang/renderpearl/api/device/GpuDevice;createBuffer(Ljava/util/function/Supplier;IJ)Lcom/mojang/renderpearl/api/buffers/GpuBuffer;"))
    private GpuBuffer horizon$allocation(GpuDevice device, Supplier<String> label, int usage, long bytes) {
        boolean capacityFits = vertexGpuBuffer != null && vertexGpuBuffer.size() >= bytes;
        long started = System.nanoTime();
        var buffer = device.createBuffer(label, usage, bytes);
        MacBenchmarkIntegration.profileVboAllocation(bytes, started, System.nanoTime() - started, capacityFits);
        return buffer;
    }
}

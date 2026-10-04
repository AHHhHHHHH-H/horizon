package dev.horizonlab.dispatch.fixture;

import com.mojang.renderpearl.api.buffers.GpuBufferSlice;
import com.mojang.renderpearl.api.commands.CommandEncoder;
import com.seibel.distanthorizons.common.render.blaze.wrappers.uniform.BlazeUniformBufferWrapper;
import java.nio.ByteBuffer;
import org.spongepowered.asm.mixin.Mixin;
import org.spongepowered.asm.mixin.injection.At;
import org.spongepowered.asm.mixin.injection.Redirect;

/** Give each experimental draw its own transient uniform snapshot. */
@Mixin(BlazeUniformBufferWrapper.class)
public abstract class MacDhCloudUniformMixin {
    @Redirect(method = "finishAndUpload", at = @At(value = "INVOKE", target =
        "Lcom/mojang/renderpearl/api/commands/CommandEncoder;writeToBuffer(Lcom/mojang/renderpearl/api/buffers/GpuBufferSlice;Ljava/nio/ByteBuffer;)V"))
    private void horizon$snapshot(CommandEncoder encoder, GpuBufferSlice destination, ByteBuffer data) {
        var batch = dev.horizonlab.metrics.MacBenchmarkIntegration.MacCloudBatchState.current;
        if (batch != null && batch.uniformWrapper == (Object) this) batch.captureUniform(data);
        else encoder.writeToBuffer(destination, data);
    }
}

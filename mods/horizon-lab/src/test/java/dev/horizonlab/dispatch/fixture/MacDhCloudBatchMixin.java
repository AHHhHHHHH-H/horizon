package dev.horizonlab.dispatch.fixture;

import com.mojang.blaze3d.systems.RenderSystem;
import com.mojang.renderpearl.api.pipeline.RenderPipeline;
import com.seibel.distanthorizons.common.render.blaze.BlazeDhGenericObjectRenderer;
import com.seibel.distanthorizons.common.render.blaze.wrappers.uniform.BlazeUniformBufferWrapper;
import com.seibel.distanthorizons.core.render.RenderParams;
import com.seibel.distanthorizons.core.render.renderer.RenderableBoxGroup;
import com.seibel.distanthorizons.core.wrapperInterfaces.minecraft.IProfilerWrapper;
import dev.horizonlab.metrics.MacBenchmarkIntegration;
import dev.horizonlab.metrics.MacBenchmarkIntegration.MacCloudBatchState;
import org.spongepowered.asm.mixin.Final;
import org.spongepowered.asm.mixin.Mixin;
import org.spongepowered.asm.mixin.Shadow;
import org.spongepowered.asm.mixin.injection.At;
import org.spongepowered.asm.mixin.injection.Inject;
import org.spongepowered.asm.mixin.injection.callback.CallbackInfo;

/** Private experiment on the pinned DH renderer; not part of playable packs. */
@Mixin(BlazeDhGenericObjectRenderer.class)
public abstract class MacDhCloudBatchMixin {
    @Shadow @Final private BlazeUniformBufferWrapper vertUniformBufferWrapper;
    @Shadow private RenderPipeline pipeline;

    @Inject(method = "render", at = @At("HEAD"))
    private void horizon$cloudBatch(RenderParams params, IProfilerWrapper profiler, boolean ssao, CallbackInfo callback) {
        var device = RenderSystem.getDevice().getDeviceInfo();
        if (!MacBenchmarkIntegration.cloudBatchEnabled || !device.backendName().equals("Vulkan") || !device.name().contains("Apple M2 Pro")) return;
        if (MacCloudBatchState.current != null) throw new AssertionError("Nested private DH batch");
        MacCloudBatchState.current = new MacCloudBatchState(vertUniformBufferWrapper);
    }

    @Inject(method = "render", at = @At("RETURN"))
    private void horizon$endBatch(RenderParams params, IProfilerWrapper profiler, boolean ssao, CallbackInfo callback) {
        var batch = MacCloudBatchState.current;
        if (batch != null) {
            try { batch.close(); }
            finally { MacCloudBatchState.current = null; }
        }
    }

    @Inject(method = "renderBoxGroupInstanced", at = @At("HEAD"), cancellable = true)
    private void horizon$drawCloud(RenderParams params, RenderableBoxGroup group,
                                  IProfilerWrapper profiler, CallbackInfo callback) {
        var batch = MacCloudBatchState.current;
        if (batch != null) {
            batch.draw(params, group, pipeline);
            callback.cancel();
        }
    }

}

package dev.horizonlab.mixin;

import com.mojang.blaze3d.systems.RenderSystem;
import com.mojang.renderpearl.api.pipeline.RenderPipeline;
import com.seibel.distanthorizons.common.render.blaze.BlazeDhGenericObjectRenderer;
import com.seibel.distanthorizons.common.render.blaze.wrappers.uniform.BlazeUniformBufferWrapper;
import com.seibel.distanthorizons.core.render.RenderParams;
import com.seibel.distanthorizons.core.render.renderer.RenderableBoxGroup;
import com.seibel.distanthorizons.core.wrapperInterfaces.minecraft.IProfilerWrapper;
import dev.horizonlab.render.DhCloudBatch;
import dev.horizonlab.render.CloudBatchStatus;
import com.llamalad7.mixinextras.injector.wrapmethod.WrapMethod;
import com.llamalad7.mixinextras.injector.wrapoperation.Operation;
import org.spongepowered.asm.mixin.Final;
import org.spongepowered.asm.mixin.Mixin;
import org.spongepowered.asm.mixin.Shadow;
import org.spongepowered.asm.mixin.injection.At;
import org.spongepowered.asm.mixin.injection.Inject;
import org.spongepowered.asm.mixin.injection.callback.CallbackInfo;

/** Opt-in M2 Pro cloud batching on the inspected DH renderer. */
@Mixin(BlazeDhGenericObjectRenderer.class)
public abstract class DhCloudBatchMixin {
    @Shadow @Final private BlazeUniformBufferWrapper vertUniformBufferWrapper;
    @Shadow private RenderPipeline pipeline;

    @WrapMethod(method = "render")
    private void horizon$cloudBatch(RenderParams params, IProfilerWrapper profiler, boolean ssao,
                                   Operation<Void> original) {
        var device = RenderSystem.getDevice().getDeviceInfo();
        boolean active = HorizonMixinPlugin.cloudBatchEnabled() && device.backendName().equals("Vulkan")
            && device.name().equals("Apple M2 Pro");
        CloudBatchStatus.active = active;
        if (!active) {
            original.call(params, profiler, ssao);
            return;
        }
        var parent = DhCloudBatch.current;
        if (parent != null) {
            parent.closePass();
            DhCloudBatch.current = null;
            try { original.call(params, profiler, ssao); }
            finally { DhCloudBatch.current = parent; }
            return;
        }
        var batch = new DhCloudBatch(vertUniformBufferWrapper);
        DhCloudBatch.current = batch;
        try (batch) { original.call(params, profiler, ssao); }
        finally { DhCloudBatch.current = null; }
    }

    @Inject(method = "renderBoxGroupInstanced", at = @At("HEAD"), cancellable = true)
    private void horizon$drawCloud(RenderParams params, RenderableBoxGroup group,
                                  IProfilerWrapper profiler, CallbackInfo callback) {
        var batch = DhCloudBatch.current;
        if (batch != null) {
            batch.draw(params, group, pipeline);
            callback.cancel();
        }
    }

}

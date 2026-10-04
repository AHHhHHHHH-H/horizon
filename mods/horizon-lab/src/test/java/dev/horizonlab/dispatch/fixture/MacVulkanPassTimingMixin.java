package dev.horizonlab.dispatch.fixture;

import com.mojang.renderpearl.api.commands.RenderPassDescriptor;
import com.mojang.renderpearl.backend.api.CommandEncoderBackend;
import com.mojang.renderpearl.backend.api.RenderPassBackend;
import com.mojang.renderpearl.backend.vulkan.VulkanCommandEncoder;
import dev.horizonlab.metrics.MacBenchmarkIntegration;
import org.spongepowered.asm.mixin.Mixin;
import org.spongepowered.asm.mixin.injection.At;
import org.spongepowered.asm.mixin.injection.Inject;
import org.spongepowered.asm.mixin.injection.callback.CallbackInfo;
import org.spongepowered.asm.mixin.injection.callback.CallbackInfoReturnable;

/** Private pass-boundary queries around the original operations. */
@Mixin(VulkanCommandEncoder.class)
public abstract class MacVulkanPassTimingMixin {
    @Inject(method = "createRenderPass", at = @At("HEAD"), require = 1)
    private void horizon$beginPass(RenderPassDescriptor descriptor, CallbackInfoReturnable<RenderPassBackend> callback) {
        MacBenchmarkIntegration.profilePassStart((CommandEncoderBackend) (Object) this, descriptor);
    }

    @Inject(method = "submitRenderPass", at = @At("RETURN"), require = 1)
    private void horizon$endPass(CallbackInfo callback) {
        MacBenchmarkIntegration.profilePassEnd((CommandEncoderBackend) (Object) this);
    }
}

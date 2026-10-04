package dev.horizonlab.dispatch.fixture;

import com.mojang.renderpearl.frontend.FrontendCommandEncoder;
import dev.horizonlab.metrics.MacBenchmarkIntegration.MacCloudBatchState;
import org.spongepowered.asm.mixin.Mixin;
import org.spongepowered.asm.mixin.injection.At;
import org.spongepowered.asm.mixin.injection.Inject;
import org.spongepowered.asm.mixin.injection.callback.CallbackInfo;
import org.spongepowered.asm.mixin.injection.callback.CallbackInfoReturnable;

/** Preserve command order when uploads or callbacks interrupt a private batch. */
@Mixin(FrontendCommandEncoder.class)
public abstract class MacDhCloudBarrierMixin {
    @Inject(method = {"submit", "clearColorTexture", "clearColorAndDepthTextures", "clearDepthTexture",
        "writeToBuffer", "copyToBuffer", "writeToTexture", "copyBufferToTexture", "copyTextureToBuffer",
        "copyTextureToTexture", "writeTimestamp"}, at = @At("HEAD"))
    private void horizon$beforeCommand(CallbackInfo callback) {
        var batch = MacCloudBatchState.current;
        if (batch != null) batch.closePass();
    }

    @Inject(method = {"createRenderPass", "createFence"}, at = @At("HEAD"))
    private void horizon$beforeResource(CallbackInfoReturnable<?> callback) {
        var batch = MacCloudBatchState.current;
        if (batch != null) batch.closePass();
    }
}

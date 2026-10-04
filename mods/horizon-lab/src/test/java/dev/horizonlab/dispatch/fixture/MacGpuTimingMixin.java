package dev.horizonlab.dispatch.fixture;

import dev.horizonlab.metrics.MacBenchmarkIntegration;
import net.minecraft.client.renderer.GameRenderer;
import org.spongepowered.asm.mixin.Mixin;
import org.spongepowered.asm.mixin.injection.At;
import org.spongepowered.asm.mixin.injection.Inject;
import org.spongepowered.asm.mixin.injection.callback.CallbackInfo;

/** Optional private asynchronous device timestamps; absent from timing-only runs. */
@Mixin(GameRenderer.class)
public abstract class MacGpuTimingMixin {
    @Inject(method = "render", at = @At("HEAD"), require = 1)
    private void horizon$beginGpuProfile(CallbackInfo callback) {
        MacBenchmarkIntegration.profileRenderStart();
    }

    @Inject(method = "render", at = @At("RETURN"), require = 1)
    private void horizon$endGpuProfile(CallbackInfo callback) {
        MacBenchmarkIntegration.profileRenderEnd();
    }
}

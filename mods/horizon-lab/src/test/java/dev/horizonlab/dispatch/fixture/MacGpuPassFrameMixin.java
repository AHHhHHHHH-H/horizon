package dev.horizonlab.dispatch.fixture;

import dev.horizonlab.metrics.MacBenchmarkIntegration;
import net.minecraft.client.Minecraft;
import org.spongepowered.asm.mixin.Mixin;
import org.spongepowered.asm.mixin.injection.At;
import org.spongepowered.asm.mixin.injection.Inject;
import org.spongepowered.asm.mixin.injection.callback.CallbackInfo;

@Mixin(Minecraft.class)
public abstract class MacGpuPassFrameMixin {
    @Inject(method = "runTick", at = @At("HEAD"), require = 1)
    private void horizon$beginPassFrame(boolean advanceGameTime, CallbackInfo callback) {
        MacBenchmarkIntegration.profileFrameStart();
    }

    @Inject(method = "runTick", at = @At("RETURN"), require = 1)
    private void horizon$endPassFrame(boolean advanceGameTime, CallbackInfo callback) {
        MacBenchmarkIntegration.profileFrameEnd();
    }
}

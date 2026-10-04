package dev.horizonlab.dispatch.fixture;

import dev.horizonlab.metrics.MacBenchmarkIntegration;
import net.minecraft.client.Minecraft;
import org.spongepowered.asm.mixin.Mixin;
import org.spongepowered.asm.mixin.injection.At;
import org.spongepowered.asm.mixin.injection.Inject;
import org.spongepowered.asm.mixin.injection.callback.CallbackInfo;

/** Private diagnostic intervals on the same boundary as the frame recorder. */
@Mixin(Minecraft.class)
public abstract class MacFrameIntervalMixin {
    @Inject(method = "runTick", at = @At("HEAD"))
    private void horizon$frameInterval(boolean advanceGameTime, CallbackInfo callback) {
        MacBenchmarkIntegration.profileFrameInterval();
    }
}

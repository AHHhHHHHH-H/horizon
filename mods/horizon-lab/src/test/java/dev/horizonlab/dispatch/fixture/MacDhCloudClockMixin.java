package dev.horizonlab.dispatch.fixture;

import com.seibel.distanthorizons.core.render.renderer.CloudRenderHandler;
import dev.horizonlab.metrics.MacBenchmarkIntegration;
import org.spongepowered.asm.mixin.Mixin;
import org.spongepowered.asm.mixin.injection.At;
import org.spongepowered.asm.mixin.injection.Redirect;

/** Freeze wall-clock cloud movement only during private image validation. */
@Mixin(CloudRenderHandler.class)
public abstract class MacDhCloudClockMixin {
    @Redirect(method = "preRender", at = @At(value = "INVOKE", target = "Ljava/lang/System;currentTimeMillis()J"))
    private long horizon$validationClock() {
        long frozen = MacBenchmarkIntegration.cloudValidationClock;
        return frozen == 0 ? System.currentTimeMillis() : frozen;
    }
}

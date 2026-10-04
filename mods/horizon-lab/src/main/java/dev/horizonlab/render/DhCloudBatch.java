package dev.horizonlab.render;

import com.mojang.blaze3d.systems.RenderSystem;
import com.mojang.renderpearl.api.buffers.GpuBuffer;
import com.mojang.renderpearl.api.buffers.GpuBufferSlice;
import com.mojang.renderpearl.api.commands.RenderPass;
import com.mojang.renderpearl.api.pipeline.IndexType;
import com.mojang.renderpearl.api.pipeline.RenderPipeline;
import com.seibel.distanthorizons.common.render.blaze.BlazeDhMetaRenderer;
import com.seibel.distanthorizons.common.render.blaze.objects.BlazeGenericObjectVertexContainer;
import com.seibel.distanthorizons.common.render.blaze.wrappers.uniform.BlazeUniformBufferWrapper;
import com.seibel.distanthorizons.common.wrappers.misc.LightMapWrapper;
import com.seibel.distanthorizons.core.render.RenderParams;
import com.seibel.distanthorizons.core.render.renderer.RenderableBoxGroup;
import java.nio.ByteBuffer;
import java.util.Optional;
import java.util.OptionalDouble;

/** Render-thread-only state, scoped to one original generic-render invocation. */
public final class DhCloudBatch implements AutoCloseable {
    public static DhCloudBatch current;
    public final BlazeUniformBufferWrapper uniformWrapper;
    private GpuBufferSlice uniform;
    private RenderPass pass;
    private Object lightmap;

    public DhCloudBatch(BlazeUniformBufferWrapper wrapper) {
        uniformWrapper = wrapper;
    }

    public void captureUniform(ByteBuffer data) {
        var copy = data.duplicate();
        copy.position(0);
        var encoder = RenderSystem.getDevice().createCommandEncoder();
        try (var mapped = encoder.transientMemory().allocateGpuMapped(copy.remaining(), 256, GpuBuffer.USAGE_UNIFORM)) {
            mapped.data().put(copy);
            uniform = mapped.slice();
        }
    }

    public void draw(RenderParams params, RenderableBoxGroup group, RenderPipeline pipeline) {
        if (uniform == null) throw new AssertionError("Missing DH uniform snapshot");
        var vertices = (BlazeGenericObjectVertexContainer) group.vertexBufferContainer;
        boolean cloud = group.getResourceLocationNamespace().equals("DistantHorizons")
            && group.getResourceLocationPath().equals("Clouds");
        if (!cloud || lightmap != params.lightmap) closePass();
        if (pass == null) {
            var targets = BlazeDhMetaRenderer.INSTANCE;
            pass = RenderSystem.getDevice().createCommandEncoder().createRenderPass(
                () -> "horizon:DH cloud batch", targets.dhColorTextureWrapper.getTextureView(),
                Optional.empty(), targets.dhDepthTextureWrapper.getTextureView(), OptionalDouble.empty());
            pass.setPipeline(RenderSystem.getCompiledPipeline(pipeline));
            var light = ((LightMapWrapper) params.lightmap).getTextureViewWrapper();
            pass.setUniform("uLightMap", light.getTextureView(), light.getTextureSampler());
            lightmap = params.lightmap;
            CloudBatchStatus.passes++;
        }
        pass.setUniform("vertUniformBlock", uniform);
        pass.setIndexBuffer(vertices.indexGpuBuffer, IndexType.INT);
        pass.setVertexBuffer(0, vertices.vboGpuBuffer.slice());
        if (vertices.uploadedBoxCount > 0) {
            pass.drawIndexed(Math.multiplyExact(vertices.uploadedBoxCount, 36), 1, 0, 0, 0);
            CloudBatchStatus.draws++;
        }
        uniform = null;
        if (!cloud) closePass();
    }

    public void closePass() {
        var closing = pass;
        pass = null;
        lightmap = null;
        if (closing != null) closing.close();
    }

    @Override public void close() {
        closePass();
    }
}

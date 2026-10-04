package dev.horizonlab.metrics;

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
import com.google.gson.JsonObject;
import com.google.gson.JsonParser;
import com.mojang.blaze3d.systems.RenderSystem;
import com.mojang.renderpearl.api.commands.GpuQueryPool;
import com.mojang.renderpearl.api.commands.RenderPassDescriptor;
import com.mojang.renderpearl.backend.api.CommandEncoderBackend;
import java.nio.file.Files;
import java.nio.file.Path;
import net.fabricmc.api.ClientModInitializer;
import net.fabricmc.fabric.api.client.event.lifecycle.v1.ClientTickEvents;
import net.fabricmc.loader.api.FabricLoader;
import net.minecraft.client.Screenshot;
import net.minecraft.client.gui.screens.TitleScreen;
import net.minecraft.network.chat.contents.TranslatableContents;
import net.minecraft.server.MinecraftServer;
import net.minecraft.world.level.levelgen.WorldOptions;
import net.minecraft.world.level.levelgen.presets.WorldPresets;
import net.minecraft.world.level.GameType;

/** Private physical-Mac measurement fixture; never included in playable packs. */
public final class MacBenchmarkIntegration implements ClientModInitializer {
    private boolean opening, finished, readbackRequested;
    private long ready, started;
    private int warmupSeconds, seconds, postRecordSeconds;
    private float yaw, pitch;
    private String label;
    private boolean gpuProfile;
    private boolean jfrProfile;
    private boolean vboProfile;
    private boolean renderClassPrewarm;
    private static final String[] PREWARM_RENDER_CLASSES = {
        "net.minecraft.client.renderer.extract.TransientBlock",
        "net.minecraft.client.renderer.block.MovingBlockRenderState",
        "net.caffeinemc.mods.sodium.client.render.frapi.render.NonTerrainBlockRenderContext",
        "net.caffeinemc.mods.sodium.client.model.light.data.SingleBlockLightDataCache",
        "it.unimi.dsi.fastutil.longs.Long2IntLinkedOpenHashMap"
    };
    private static VboTiming vboTiming;
    private jdk.jfr.Recording flightRecording;
    private static long flightDeadline, previousFlightFrame;
    private static FrameIntervalEvent flightFrame;
    private boolean passProfile;
    private boolean sceneRequested;
    private String scene;
    public static boolean cloudBatchEnabled;
    public static long cloudValidationClock;
    private boolean cloudImageCheck, imagePending, productionCloudBatch;
    private long imageDrawsStart, imagePassesStart;
    private int imageStage;
    private long imageReady;
    private static GpuTiming gpuTiming;
    private static PassTiming passTiming;

    public static void profileVboAllocation(long bytes, long start, long duration, boolean capacityFits) {
        if (vboTiming != null) vboTiming.add(bytes, start, duration, capacityFits);
    }

    public static void profileVboUpload(long bytes, long start, long duration, boolean copy) {
        if (vboTiming != null && vboTiming.started != 0 && start >= vboTiming.started && start < vboTiming.deadline) {
            (copy ? vboTiming.copy : vboTiming.upload).add(bytes, start - vboTiming.started, duration);
        }
    }

    @jdk.jfr.Name("dev.horizonlab.TerrainUpload")
    @jdk.jfr.Label("Private DH terrain upload")
    @jdk.jfr.StackTrace(false)
    public static final class TerrainUploadEvent extends jdk.jfr.Event {
        public long bytes;
    }

    public static TerrainUploadEvent beginTerrainUpload(long bytes) {
        if (flightDeadline == 0 || System.nanoTime() >= flightDeadline) return null;
        var event = new TerrainUploadEvent();
        event.bytes = bytes;
        event.begin();
        return event;
    }

    private static final class UploadTiming {
        private final long[] durations = new long[300_000];
        private final com.google.gson.JsonArray slow = new com.google.gson.JsonArray();
        private int count, dropped, slowDropped;
        private long bytes;

        void add(long size, long offset, long duration) {
            bytes += size;
            if (count == durations.length) { dropped++; return; }
            durations[count++] = duration;
            if (duration < 1_000_000) return;
            if (slow.size() >= 128) { slowDropped++; return; }
            var event = new JsonObject();
            event.addProperty("recording_offset_ns", offset);
            event.addProperty("duration_ns", duration);
            event.addProperty("bytes", size);
            slow.add(event);
        }

        JsonObject summary() {
            var result = new JsonObject();
            result.addProperty("bytes", bytes);
            result.addProperty("dropped_samples", dropped);
            result.addProperty("dropped_slow_events", slowDropped);
            result.add("duration", new com.google.gson.Gson().toJsonTree(
                SampleStats.of(java.util.Arrays.copyOf(durations, count), count)));
            result.add("slow_events", slow);
            return result;
        }
    }

    private static final class VboTiming {
        private final UploadTiming upload = new UploadTiming(), copy = new UploadTiming();
        private final long[] durations = new long[300_000];
        private final com.google.gson.JsonArray slow = new com.google.gson.JsonArray();
        private long started, deadline, totalCalls, bytesAllocated, capacityReuseCandidates;
        private int count, dropped, slowDropped;

        void add(long bytes, long start, long duration, boolean capacityFits) {
            totalCalls++;
            if (started == 0 || start < started || start >= deadline) return;
            bytesAllocated += bytes;
            if (capacityFits) capacityReuseCandidates++;
            if (count == durations.length) { dropped++; return; }
            durations[count++] = duration;
            if (duration < 5_000_000) return;
            if (slow.size() >= 128) { slowDropped++; return; }
            var event = new JsonObject();
            event.addProperty("recording_offset_ns", start - started);
            event.addProperty("duration_ns", duration);
            event.addProperty("bytes", bytes);
            event.addProperty("existing_capacity_fits", capacityFits);
            slow.add(event);
        }

        JsonObject summary() {
            var result = new JsonObject();
            result.addProperty("instrumented_calls_total", totalCalls);
            result.addProperty("bytes_allocated_during_recording", bytesAllocated);
            result.addProperty("capacity_reuse_candidates", capacityReuseCandidates);
            result.addProperty("dropped_samples", dropped);
            result.addProperty("dropped_slow_events", slowDropped);
            result.add("allocation_duration", new com.google.gson.Gson().toJsonTree(
                SampleStats.of(java.util.Arrays.copyOf(durations, count), count)));
            result.add("slow_allocations", slow);
            result.add("whole_upload", upload.summary());
            result.add("staging_copy_and_barrier", copy.summary());
            result.addProperty("note", "Original DH destination allocation, complete upload, and staging/copy/barrier timings. Nested durations are not additive. Profiling is separate from ordinary FPS measurements.");
            return result;
        }
    }

    @jdk.jfr.Name("dev.horizonlab.FrameInterval")
    @jdk.jfr.Label("Private Minecraft frame interval")
    @jdk.jfr.StackTrace(false)
    public static final class FrameIntervalEvent extends jdk.jfr.Event {
        @jdk.jfr.Timespan(jdk.jfr.Timespan.NANOSECONDS)
        public long intervalNs;
    }

    public static void profileFrameInterval() {
        long now = System.nanoTime();
        if (flightDeadline == 0 || now >= flightDeadline) { flightFrame = null; return; }
        if (flightFrame != null) {
            flightFrame.end();
            flightFrame.intervalNs = now - previousFlightFrame;
            flightFrame.commit();
        }
        flightFrame = new FrameIntervalEvent();
        flightFrame.begin();
        previousFlightFrame = now;
    }

    public static void profileFrameStart() { if (passTiming != null) passTiming.beginFrame(); }
    public static void profileFrameEnd() { if (passTiming != null) passTiming.endFrame(); }
    public static void profilePassStart(CommandEncoderBackend encoder, RenderPassDescriptor descriptor) {
        if (passTiming != null) passTiming.begin(encoder, descriptor.label().get());
    }
    public static void profilePassEnd(CommandEncoderBackend encoder) {
        if (passTiming != null) passTiming.end(encoder);
    }

    private static final class PassTiming implements AutoCloseable {
        private final GpuQueryPool pool = RenderSystem.getDevice().createTimestampQueryPool(512);
        private final String[] pending = new String[256];
        private final int[] pendingFrames = new int[256];
        private final java.util.Map<String, java.util.ArrayList<Long>> durations = new java.util.LinkedHashMap<>();
        private final java.util.Map<String, java.util.Map<Integer, Long>> frameDurations = new java.util.LinkedHashMap<>();
        private final java.util.Map<String, Long> passCounts = new java.util.LinkedHashMap<>();
        private final long deadlineNs;
        private final double period = RenderSystem.getDevice().getDeviceInfo().timestampPeriod();
        private int frame, active = -1, skipped, invalid;
        private boolean sampling;
        private String activeLabel;

        PassTiming(long deadlineNs) { this.deadlineNs = deadlineNs; }

        private void collect() {
            for (int slot = 0; slot < pending.length; slot++) {
                String label = pending[slot];
                if (label == null) continue;
                var values = pool.getValues(slot * 2, 2);
                if (values[0].isEmpty() || values[1].isEmpty()) continue;
                pending[slot] = null;
                long duration = (long) ((values[1].getAsLong() - values[0].getAsLong()) * period);
                if (duration <= 0) { invalid++; continue; }
                if (!durations.containsKey(label) && durations.size() >= 64) { skipped++; continue; }
                var samples = durations.computeIfAbsent(label, key -> new java.util.ArrayList<>());
                if (samples.size() < 4096) samples.add(duration);
                var frames = frameDurations.computeIfAbsent(label, key -> new java.util.LinkedHashMap<>());
                int sampleFrame = pendingFrames[slot];
                if (frames.containsKey(sampleFrame) || frames.size() < 2048) {
                    frames.merge(sampleFrame, duration, Long::sum);
                    passCounts.merge(label, 1L, Long::sum);
                } else skipped++;
            }
        }

        void beginFrame() {
            collect();
            sampling = System.nanoTime() < deadlineNs && frame++ % 20 == 0;
        }

        void endFrame() {
            sampling = false;
            if (active >= 0) throw new AssertionError("Unclosed private GPU pass timing");
        }

        void begin(CommandEncoderBackend encoder, String label) {
            if (!sampling) return;
            if (active >= 0) throw new AssertionError("Overlapping private GPU pass timing");
            for (int slot = 0; slot < pending.length; slot++) {
                if (pending[slot] != null) continue;
                encoder.writeTimestamp(pool, slot * 2);
                activeLabel = label;
                pendingFrames[slot] = frame;
                active = slot;
                return;
            }
            skipped++;
        }

        void end(CommandEncoderBackend encoder) {
            if (active < 0) return;
            int slot = active;
            active = -1;
            encoder.writeTimestamp(pool, slot * 2 + 1);
            pending[slot] = activeLabel;
        }

        JsonObject summary() {
            collect();
            var result = new JsonObject();
            var passes = new JsonObject();
            var gson = new com.google.gson.Gson();
            durations.forEach((label, values) -> {
                long[] samples = values.stream().mapToLong(Long::longValue).toArray();
                passes.add(label, gson.toJsonTree(SampleStats.of(samples, samples.length)));
            });
            result.add("passes", passes);
            var totals = new JsonObject();
            frameDurations.forEach((label, frames) -> {
                long[] samples = frames.values().stream().mapToLong(Long::longValue).toArray();
                var total = gson.toJsonTree(SampleStats.of(samples, samples.length)).getAsJsonObject();
                total.addProperty("pass_invocations", passCounts.get(label));
                totals.add(label, total);
            });
            result.add("per_sampled_frame", totals);
            result.addProperty("sampling_frame_stride", 20);
            result.addProperty("skipped_queries", skipped);
            result.addProperty("invalid_queries", invalid);
            return result;
        }

        @Override public void close() { pool.close(); }
    }

    /** Called only by the optional private GPU timing mixin. */
    public static void profileRenderStart() {
        if (gpuTiming != null) gpuTiming.begin();
    }

    public static void profileRenderEnd() {
        if (gpuTiming != null) gpuTiming.end();
    }

    private static final class GpuTiming implements AutoCloseable {
        private final GpuQueryPool pool = RenderSystem.getDevice().createTimestampQueryPool(16);
        private final boolean[] pending = new boolean[8];
        private final long[] gpuSamples = new long[120_000];
        private final long[] cpuSamples = new long[120_000];
        private final float period = RenderSystem.getDevice().getDeviceInfo().timestampPeriod();
        private final long deadlineNs;
        private int active = -1, gpuCount, cpuCount, skipped, invalid;
        private long cpuStart;
        private boolean recordingFrame;

        GpuTiming(long deadlineNs) { this.deadlineNs = deadlineNs; }

        private void collect() {
            for (int slot = 0; slot < pending.length; slot++) {
                if (!pending[slot]) continue;
                var values = pool.getValues(slot * 2, 2);
                if (values[0].isEmpty() || values[1].isEmpty()) continue;
                pending[slot] = false;
                long duration = (long) ((values[1].getAsLong() - values[0].getAsLong()) * (double) period);
                if (duration > 0 && gpuCount < gpuSamples.length) gpuSamples[gpuCount++] = duration;
                else invalid++;
            }
        }

        void begin() {
            recordingFrame = System.nanoTime() < deadlineNs;
            if (!recordingFrame) return;
            collect();
            cpuStart = System.nanoTime();
            active = -1;
            for (int slot = 0; slot < pending.length; slot++) {
                if (pending[slot]) continue;
                active = slot;
                RenderSystem.getDevice().createCommandEncoder().writeTimestamp(pool, slot * 2);
                return;
            }
            skipped++;
        }

        void end() {
            if (!recordingFrame) return;
            long duration = System.nanoTime() - cpuStart;
            if (duration > 0 && cpuCount < cpuSamples.length) cpuSamples[cpuCount++] = duration;
            if (active < 0) return;
            RenderSystem.getDevice().createCommandEncoder().writeTimestamp(pool, active * 2 + 1);
            pending[active] = true;
            active = -1;
        }

        JsonObject summary() {
            collect();
            var gson = new com.google.gson.Gson();
            var result = new JsonObject();
            result.addProperty("status", gpuCount > 0 ? "available" : "unavailable");
            result.add("gpu_render_duration", gson.toJsonTree(SampleStats.of(gpuSamples, gpuCount)));
            result.add("cpu_render_call_duration", gson.toJsonTree(SampleStats.of(cpuSamples, cpuCount)));
            result.addProperty("skipped_queries", skipped);
            result.addProperty("invalid_or_overflowed_queries", invalid);
            int pendingCount = 0;
            for (boolean value : pending) if (value) pendingCount++;
            result.addProperty("pending_queries_at_end", pendingCount);
            result.addProperty("timestamp_period_ns", period);
            return result;
        }

        @Override public void close() { pool.close(); }
    }

    @Override public void onInitializeClient() {
        try {
            var config = new java.util.Properties();
            try (var reader = Files.newBufferedReader(FabricLoader.getInstance().getConfigDir()
                    .resolve("mac-benchmark.properties"))) { config.load(reader); }
            warmupSeconds = Integer.parseInt(config.getProperty("warmupSeconds", "60"));
            seconds = Integer.parseInt(config.getProperty("seconds", "120"));
            yaw = Float.parseFloat(config.getProperty("yaw", "141.5"));
            pitch = Float.parseFloat(config.getProperty("pitch", "16.95"));
            label = config.getProperty("label", "mac-stationary");
            productionCloudBatch = Boolean.parseBoolean(config.getProperty("productionCloudBatch", "false"));
            cloudImageCheck = Boolean.parseBoolean(config.getProperty("cloudImageCheck", "false"));
            cloudBatchEnabled = !cloudImageCheck && Boolean.parseBoolean(config.getProperty("cloudBatch", "false"));
            gpuProfile = Boolean.parseBoolean(config.getProperty("gpuProfile", "false"));
            jfrProfile = Boolean.parseBoolean(config.getProperty("jfrProfile", "false"));
            vboProfile = Boolean.parseBoolean(config.getProperty("vboProfile", "false"));
            if (vboProfile) vboTiming = new VboTiming();
            renderClassPrewarm = Boolean.parseBoolean(config.getProperty("renderClassPrewarm", "false"));
            postRecordSeconds = Integer.parseInt(config.getProperty("postRecordSeconds", "3"));
            if (renderClassPrewarm) {
                for (String type : PREWARM_RENDER_CLASSES) {
                    Class.forName(type, false, MacBenchmarkIntegration.class.getClassLoader());
                }
                System.out.println("HORIZON_MAC_RENDER_CLASSES_PREWARMED " + PREWARM_RENDER_CLASSES.length);
            }
            passProfile = Boolean.parseBoolean(config.getProperty("passProfile", "false"));
            scene = config.getProperty("scene", "spawn");
            if (warmupSeconds < 0 || warmupSeconds > 300 || seconds < 10 || seconds > 120
                    || !Float.isFinite(yaw) || !Float.isFinite(pitch)
                    || !label.matches("[a-zA-Z0-9_-]{1,64}")
                    || !(scene.equals("spawn") || scene.equals("overlook") || scene.equals("flyover"))) throw new IllegalArgumentException("Invalid fixture settings");
        } catch (Exception error) { throw new IllegalStateException("Private Mac benchmark configuration failed", error); }
        ClientTickEvents.END_CLIENT_TICK.register(client -> {
            if (finished || client.gui.overlay() != null) return;
            if (!opening && client.gui.screen() instanceof TitleScreen screen) {
                opening = true;
                if (Files.isRegularFile(client.gameDirectory.toPath().resolve("saves/Demo_World/level.dat"))) {
                    client.createWorldOpenFlows().openWorld("Demo_World", () -> {
                        throw new AssertionError("Private benchmark world did not open");
                    });
                } else {
                    client.createWorldOpenFlows().createFreshLevel("Demo_World", MinecraftServer.DEMO_SETTINGS,
                        WorldOptions.DEMO_OPTIONS, WorldPresets::createNormalWorldDimensions, screen);
                }
                return;
            }
            if (client.gui.screen() != null && client.gui.screen().getTitle().getContents() instanceof TranslatableContents title
                    && title.getKey().equals("demo.help.title")) { client.gui.setScreen(null); return; }
            if (client.level == null || client.player == null || client.gui.screen() != null
                    || client.isPaused() || !client.isWindowActive()) {
                if (started == 0) ready = 0;
                return;
            }
            if (!scene.equals("spawn")) {
                if (!sceneRequested) {
                    sceneRequested = true;
                    var server = client.getSingleplayerServer();
                    var playerId = client.player.getUUID();
                    server.execute(() -> {
                        var player = server.getPlayerList().getPlayer(playerId);
                        player.setGameMode(GameType.SPECTATOR);
                        player.teleportTo(-41.5, 100.0, 72.5);
                        player.setDeltaMovement(0, 0, 0);
                    });
                    return;
                }
                if (!client.player.isSpectator()
                        || (started == 0 && Math.abs(client.player.getY() - 100.0) > 0.01)) return;
                if (scene.equals("overlook")) {
                    client.player.setPos(-41.5, 100.0, 72.5);
                    client.player.setDeltaMovement(0, 0, 0);
                }
            }
            // Control only this private workload's camera; simulation runs normally.
            client.player.setYRot(yaw);
            client.player.setXRot(pitch);
            if (scene.equals("overlook")) client.player.setOldPosAndRot();
            long now = System.nanoTime();
            if (scene.equals("flyover")) client.options.keyUp.setDown(
                started != 0 && now - started < seconds * 1_000_000_000L);
            if (ready == 0) {
                ready = now;
                System.out.println("HORIZON_MAC_BENCHMARK_READY");
            }
            if (jfrProfile && started == 0 && flightRecording == null
                    && now - ready >= Math.max(0, warmupSeconds - 15L) * 1_000_000_000L) {
                try {
                    flightRecording = new jdk.jfr.Recording(jdk.jfr.Configuration.getConfiguration("profile"));
                    flightRecording.setName("Private M2 Pro diagnostics");
                    flightRecording.setMaxSize(128L * 1024 * 1024);
                    flightRecording.enable(FrameIntervalEvent.class).withThreshold(java.time.Duration.ZERO);
                    if (vboProfile) flightRecording.enable(TerrainUploadEvent.class).withThreshold(java.time.Duration.ofMillis(1));
                    flightRecording.enable("jdk.ThreadPark").withThreshold(java.time.Duration.ofMillis(1));
                    flightRecording.enable("jdk.JavaMonitorEnter").withThreshold(java.time.Duration.ofMillis(1));
                    flightRecording.enable("jdk.ClassLoad").withThreshold(java.time.Duration.ofMillis(1));
                    flightRecording.start();
                    var frameEvent = new FrameIntervalEvent();
                    frameEvent.begin(); frameEvent.end();
                    if (vboProfile) {
                        // Initialize the private event machinery during warmup, before timing uploads.
                        var event = new TerrainUploadEvent();
                        event.begin(); event.end(); event.commit();
                    }
                } catch (Exception error) { throw new AssertionError("Private JFR start failed", error); }
            }
            if (cloudImageCheck) {
                if (now - ready >= warmupSeconds * 1_000_000_000L) checkCloudImages(client, now);
                return;
            }
            if (started == 0 && now - ready >= warmupSeconds * 1_000_000_000L) {
                try {
                    var method = Class.forName("dev.horizonlab.client.HorizonClient")
                        .getDeclaredMethod("start", String.class, int.class);
                    method.setAccessible(true);
                    if (!method.invoke(null, label, seconds).equals(1)) throw new AssertionError("Recording failed to start");
                    for (String[] target : new String[][]{{"dev.horizonlab.client.HorizonClient", "frames"},
                            {"dev.horizonlab.HorizonLab", "ticks"}}) {
                        var field = Class.forName(target[0]).getDeclaredField(target[1]);
                        field.setAccessible(true);
                        var recording = (Recording) field.get(null);
                        if (recording != null) synchronized (recording) {
                            recording.context.put("private_render_class_prewarm", renderClassPrewarm);
                            if (gpuProfile || passProfile || jfrProfile || vboProfile) {
                                recording.context.put("private_profiling_enabled", true);
                            }
                        }
                    }
                    started = now;
                    if (vboTiming != null) {
                        vboTiming.started = now;
                        vboTiming.deadline = now + seconds * 1_000_000_000L;
                    }
                    if (jfrProfile) flightDeadline = now + seconds * 1_000_000_000L;
                    if (gpuProfile) gpuTiming = new GpuTiming(now + seconds * 1_000_000_000L);
                    if (passProfile) passTiming = new PassTiming(now + seconds * 1_000_000_000L);
                    System.out.println("HORIZON_MAC_BENCHMARK_STARTED");
                } catch (ReflectiveOperationException error) { throw new AssertionError("Private recorder start failed", error); }
            }
            if (started == 0 || now - started < (seconds + (long) postRecordSeconds) * 1_000_000_000L || readbackRequested) return;
            Path output = client.gameDirectory.toPath().resolve("horizon-benchmarks");
            try (var paths = Files.list(output)) {
                var reports = paths.filter(p -> p.getFileName().toString().startsWith(label + "-")
                    && p.toString().endsWith(".json")).toList();
                if (reports.size() != 2) throw new AssertionError("Expected completed frame and tick reports");
                var result = new JsonObject();
                result.add("cloud_batch", MacCloudBatchState.summary());
                result.addProperty("warmup_seconds", warmupSeconds);
                result.addProperty("post_record_seconds", postRecordSeconds);
                result.addProperty("scene", scene);
                result.addProperty("render_class_prewarm", renderClassPrewarm);
                if (renderClassPrewarm) result.add("prewarmed_render_classes",
                    new com.google.gson.Gson().toJsonTree(PREWARM_RENDER_CLASSES));
                result.addProperty("camera_yaw", yaw);
                result.addProperty("camera_pitch", pitch);
                result.addProperty("player_x", client.player.getX());
                result.addProperty("player_y", client.player.getY());
                result.addProperty("player_z", client.player.getZ());
                var device = RenderSystem.getDevice().getDeviceInfo();
                result.addProperty("graphics_backend", device.backendName());
                result.addProperty("gpu", device.name());
                if (vboTiming != null) {
                    result.add("vbo_allocation_profile", vboTiming.summary());
                    vboTiming = null;
                }
                if (flightRecording != null) {
                    flightDeadline = 0;
                    flightFrame = null;
                    flightRecording.stop();
                    flightRecording.dump(client.gameDirectory.toPath().resolve("mac-benchmark.jfr"));
                    var jfr = new JsonObject();
                    jfr.addProperty("file", "mac-benchmark.jfr");
                    jfr.addProperty("frame_event", jdk.jfr.EventType.getEventType(FrameIntervalEvent.class).getName());
                    jfr.addProperty("max_recording_bytes", 128L * 1024 * 1024);
                    result.add("jfr_profile", jfr);
                    flightRecording.close();
                    flightRecording = null;
                }
                if (gpuTiming != null) {
                    result.add("gpu_profile", gpuTiming.summary());
                    gpuTiming.close();
                    gpuTiming = null;
                }
                if (passTiming != null) {
                    result.add("gpu_pass_profile", passTiming.summary());
                    passTiming.close();
                    passTiming = null;
                }
                for (Path path : reports) {
                    var report = JsonParser.parseString(Files.readString(path)).getAsJsonObject();
                    if (report.getAsJsonObject("stats").get("count").getAsInt() < 100)
                        throw new AssertionError("Too few active measurement samples");
                    result.add(report.get("kind").getAsString(), report);
                }
                readbackRequested = true;
                Screenshot.takeScreenshot(client.gameRenderer.mainRenderTarget(), image -> {
                    try (image) {
                        image.writeToFile(client.gameDirectory.toPath().resolve("mac-benchmark-private.png"));
                        result.addProperty("framebuffer_width", image.getWidth());
                        result.addProperty("framebuffer_height", image.getHeight());
                        Files.writeString(client.gameDirectory.toPath().resolve("mac-benchmark-result.json"), result.toString());
                        System.out.println("HORIZON_MAC_BENCHMARK_RESULT " + result);
                        finished = true;
                        client.execute(client::stop);
                    } catch (Exception error) { throw new AssertionError("Private Mac readback failed", error); }
                });
            } catch (Exception error) { throw new AssertionError("Private Mac benchmark failed", error); }
        });
    }

    private void setImageBatchEnabled(boolean enabled) {
        if (!productionCloudBatch) { cloudBatchEnabled = enabled; return; }
        try {
            var flag = Class.forName("dev.horizonlab.mixin.HorizonMixinPlugin").getDeclaredField("cloudBatch");
            flag.setAccessible(true);
            flag.setBoolean(null, enabled);
        } catch (ReflectiveOperationException error) { throw new AssertionError("Private production batch toggle failed", error); }
    }

    /** Frozen-scene A/B/A readbacks are separate from ordinary performance runs. */
    private void checkCloudImages(net.minecraft.client.Minecraft client, long now) {
        if (imageReady == 0) {
            setImageBatchEnabled(false);
            imageDrawsStart = productionCloudBatch ? dev.horizonlab.render.CloudBatchStatus.draws : MacCloudBatchState.totalDraws;
            imagePassesStart = productionCloudBatch ? dev.horizonlab.render.CloudBatchStatus.passes : MacCloudBatchState.totalPasses;
            cloudValidationClock = System.currentTimeMillis();
            if (!client.gui.hud.isHidden()) client.gui.hud.toggle();
            client.getSingleplayerServer().execute(() -> client.getSingleplayerServer().tickRateManager().setFrozen(true));
            imageReady = now;
            return;
        }
        if (!client.level.tickRateManager().isFrozen() || imagePending || now - imageReady < 5_000_000_000L) return;
        String name = new String[]{"cloud-image-control-a.png", "cloud-image-batched.png", "cloud-image-control-b.png"}[imageStage];
        imagePending = true;
        Screenshot.takeScreenshot(client.gameRenderer.mainRenderTarget(), image -> {
            try (image) {
                image.writeToFile(client.gameDirectory.toPath().resolve(name));
                imageStage++;
                setImageBatchEnabled(imageStage == 1);
                imageReady = System.nanoTime();
                imagePending = false;
                if (imageStage < 3) return;
                var result = new JsonObject();
                var device = RenderSystem.getDevice().getDeviceInfo();
                result.addProperty("image_check_only", true);
                result.addProperty("graphics_backend", device.backendName());
                result.addProperty("gpu", device.name());
                result.addProperty("frozen_scene", client.level.tickRateManager().isFrozen());
                result.addProperty("framebuffer_width", image.getWidth());
                result.addProperty("framebuffer_height", image.getHeight());
                result.addProperty("production_cloud_batch", productionCloudBatch);
                var batch = new JsonObject();
                batch.addProperty("draws", (productionCloudBatch ? dev.horizonlab.render.CloudBatchStatus.draws : MacCloudBatchState.totalDraws) - imageDrawsStart);
                batch.addProperty("passes", (productionCloudBatch ? dev.horizonlab.render.CloudBatchStatus.passes : MacCloudBatchState.totalPasses) - imagePassesStart);
                result.add("cloud_batch", batch);
                if (productionCloudBatch) setImageBatchEnabled(true);
                Files.writeString(client.gameDirectory.toPath().resolve("mac-benchmark-result.json"), result.toString());
                finished = true;
                client.execute(client::stop);
            } catch (Exception error) { throw new AssertionError("Private cloud image validation failed", error); }
        });
    }

    /** Render-thread-only state, scoped to one original generic-render invocation. */
    public static final class MacCloudBatchState implements AutoCloseable {
        public static MacCloudBatchState current;
        public final BlazeUniformBufferWrapper uniformWrapper;
        private GpuBufferSlice uniform;
        private RenderPass pass;
        private Object lightmap;
        private static long totalDraws, totalPasses;

        public MacCloudBatchState(BlazeUniformBufferWrapper wrapper) {
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
            if (uniform == null) throw new AssertionError("Missing private DH uniform snapshot");
            var vertices = (BlazeGenericObjectVertexContainer) group.vertexBufferContainer;
            boolean cloud = group.getResourceLocationNamespace().equals("DistantHorizons")
                && group.getResourceLocationPath().equals("Clouds");
            if (!cloud || lightmap != params.lightmap) closePass();
            if (pass == null) {
                var targets = BlazeDhMetaRenderer.INSTANCE;
                pass = RenderSystem.getDevice().createCommandEncoder().createRenderPass(
                    () -> "horizon-private:DH cloud batch", targets.dhColorTextureWrapper.getTextureView(),
                    Optional.empty(), targets.dhDepthTextureWrapper.getTextureView(), OptionalDouble.empty());
                pass.setPipeline(RenderSystem.getCompiledPipeline(pipeline));
                var light = ((LightMapWrapper) params.lightmap).getTextureViewWrapper();
                pass.setUniform("uLightMap", light.getTextureView(), light.getTextureSampler());
                lightmap = params.lightmap;
                totalPasses++;
            }
            pass.setUniform("vertUniformBlock", uniform);
            pass.setIndexBuffer(vertices.indexGpuBuffer, IndexType.INT);
            pass.setVertexBuffer(0, vertices.vboGpuBuffer.slice());
            if (vertices.uploadedBoxCount > 0) {
                pass.drawIndexed(Math.multiplyExact(vertices.uploadedBoxCount, 36), 1, 0, 0, 0);
                totalDraws++;
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

        public static JsonObject summary() {
            var result = new JsonObject();
            result.addProperty("requested", cloudBatchEnabled);
            result.addProperty("draws", totalDraws);
            result.addProperty("passes", totalPasses);
            return result;
        }

        @Override public void close() {
            closePass();
        }
    }
}

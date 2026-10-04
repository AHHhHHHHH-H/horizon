# Optimization research plan

## Starting point

The first playable Java foundation is in the root README. Current implementation
and performance work targets the physical **M2 Pro / 16 GB MacBook Pro**: 8
performance cores, 4 efficiency cores and a 19-core GPU. Windows/Linux,
NVIDIA/AMD and other Macs are deferred until this device has a convincing,
repeatable gameplay improvement. Existing portable packs remain available.

Vanilla gameplay, world formats and multiplayer behavior must be preserved;
existing modpack compatibility is not required. Any language, renderer or
architecture is in scope when measured gains justify it. Full Retina resolution,
32 detailed chunks and 128-chunk distant terrain are the user's supplied workload;
reducing those settings is a separate quality tradeoff, not proof of a faster
renderer.

The user's 4 October capture reports 84.27 average FPS, 48.18 slowest-1%-average
FPS and 18.14 ms p99 at 3456×2234, OpenGL, FOV 110 and normal simulation distance
12. About 81.59 seconds of frame intervals were accepted within 120.09 elapsed
seconds; menu, paused or unfocused frames were excluded. Tick execution averages
7.89 ms with no execution above 50 ms. Its 1,646 tick invocations correspond to
82.3 seconds at 20 TPS, so the 13.71 observed invocation rate across the whole
recording does not demonstrate an overloaded server. GPU time and movement route
are unknown. Original captures remain private under `.local/user-baseline/`.

Initial physical-Mac inspection verifies the existing Vulkan backend creates a
MoltenVK 1.4.2 device on this GPU and renders a nonuniform demo-world framebuffer.
This establishes an available Metal-backed path, not an FPS gain or complete
visual equivalence. Matched private OpenGL/Vulkan measurements come first.

A stationary 60-second warm-up exposed Minecraft's default AFK limiter: an
otherwise unlimited recording settled at 29.96 FPS. That run is rejected as a
renderer baseline. Private stationary instances now select the vanilla
`minimized` inactivity option; playable defaults retain their normal idle power
saving. Recorder metadata captures the option, effective limiter boundaries and
throttled observations. The private benchmark checks active duration, sample
files, unchanged settings and normal tick-manager state before accepting a run.

Inspect both pinned game JARs locally, retain bytecode as the reference, and measure actual Minecraft workloads before selecting a subsystem to replace. The Rust face-count experiment establishes a small correctness/benchmark workflow; it does not identify a Minecraft bottleneck.

Whole-game translation is a long-term research objective. Decompiled Java is an inspection artifact, not the original source tree: build configuration, comments, tooling history, and some source-level constructs cannot be recovered faithfully. A mechanically translated Rust game would also have to preserve behavior, data formats, networking and rendering. An isolated hot-path replacement is a useful first experiment.

## Local code map

These entry points were verified present in the decompiled 26.3 client. Paths below are relative to `.local/minecraft/26.3/sources/client/net/minecraft/`; shared/server logic can also be inspected in the dedicated-server tree.

| Area | Local entry points | Questions to measure |
| --- | --- | --- |
| Client loop | `client/Minecraft.java` | CPU frame-time distribution, allocation/GC, render-thread stalls |
| Server loop | `server/MinecraftServer.java`, `server/level/ServerLevel.java` | Tick time, entity/block tick work, waiting on chunk jobs |
| Section compilation | `client/renderer/chunk/SectionCompiler.java`, `SectionRenderDispatcher.java` in that directory | Work per rebuild, queue latency, allocations, upload overhead |
| World generation | `world/level/levelgen/NoiseChunk.java`, `NoiseBasedChunkGenerator.java` in that directory | Time per newly generated chunk, cache reuse, generation scheduling |
| Lighting | `world/level/lighting/LevelLightEngine.java` | Update propagation work and queue contention |
| Block storage | `world/level/chunk/PalettedContainer.java` | Palette access costs, locality, mutation behavior |
| Region I/O | `world/level/chunk/storage/RegionFile.java` | Serialization/compression cost versus disk waits |

The local inventory contains every class, bytecode size, method count, and source-file relationship. Large classes are navigation hints; bytecode size does not establish execution cost. Generated registries can be large while irrelevant to steady-state frame time.

## Capture a real baseline

Use a licensed launcher installation and a copy of a test world. Keep raw captures and world data in `.local/`. Record game version/hash, mod versions, Java version/flags, CPU/GPU, OS, resolution, render/simulation distance, world seed, route, and warm-up duration.

Use at least these separate scenarios:

1. A fixed stationary view after chunks settle.
2. A reproducible movement route through already generated chunks.
3. The same route through newly generated terrain.
4. A controlled entity/redstone workload on the dedicated server.
5. World loading/saving as a separate latency test.

For a running local Java process, JDK tooling can capture a bounded recording:

```sh
jcmd -l
# Replace 12345 with the Minecraft JVM PID, and use an absolute capture path.
jcmd 12345 JFR.start name=mc-baseline settings=profile duration=120s filename=/absolute/path/to/repo/.local/baseline.jfr
jfr summary .local/baseline.jfr
jfr print --events jdk.ExecutionSample,jdk.GarbageCollection .local/baseline.jfr > .local/baseline-events.txt
```

Collect CPU profiles, allocation/GC information, frame-time percentiles, server tick-time percentiles, and relevant GPU timings. JFR samples alone do not measure FPS or GPU cost. JVM compilation/warm-up, profiling overhead, world generation, and background activity must be controlled when comparing runs. The [core comparison](core-performance.md) contains stationary Linux software-rendering observations; [physical M2 Pro observations](m2-pro-performance.md) now include repeated renderer measurements and GPU pass timings. Movement/generation and populated dedicated-server measurements remain open.

## First native integration experiment

Choose a kernel only after a profile shows enough inclusive cost to justify it. A candidate should have a clear input/output boundary and deterministic reference tests. Candidate areas include occupancy preprocessing, palette scans, or batched generation calculations; these are hypotheses to investigate.

Measure an improved Java implementation first. Then compare an original Rust implementation with equivalent work and account for Java/native transition cost, data marshaling, allocation, and scheduling. Use coarse batches rather than a call per block. Preserve Minecraft behavior for boundaries, partial/transparent blocks, lighting, saved data, and deterministic world generation wherever the selected kernel touches them.

Ship any integration as an original mod/native library loaded into the user's local game. Do not bundle a modified Minecraft client/server. Add a fallback to the reference implementation and differential tests using locally generated fixtures.

## Gates before expanding scope

| Gate | Evidence required |
| --- | --- |
| Bottleneck selected | Reproducible real-game profile and workload definition |
| Kernel equivalent | Boundary cases and randomized differential tests, including real input shapes |
| Native call worthwhile | End-to-end measurements including copying and transitions |
| Game improves | Repeated real-game frame/tick distributions, allocations, memory, and CPU/GPU cost |
| More Rust justified | Improvement persists across workloads without changing gameplay |

If a subsystem occupies fraction `p` of total measured time, even eliminating its cost gives a theoretical maximum overall speedup of `1 / (1 - p)`. For example, a subsystem consuming 10% of frame time cannot alone yield a 2× whole-game speedup. Apply this before undertaking a large rewrite.

# Physical M2 Pro work

The current target is the user's M2 Pro MacBook Pro: 8 performance cores,
4 efficiency cores, 19 GPU cores and 16 GB unified memory. Work on other GPU
vendors and operating systems is deferred while this device is being optimized.
Gameplay rules, save formats, simulation distance 12 and normal 20 TPS remain
the compatibility target.

## First renderer observations

These are early measurements on the actual Apple GPU, not software rendering.
Vulkan successfully uses the bundled MoltenVK 1.4.2 Metal path; OpenGL uses
Apple's 4.1 driver. Both use the same core mods and Distant Horizons 3.3.4,
32 detailed chunks, 128-chunk medium LOD, FOV 110, 6 GiB maximum heap, G1,
VSync off and uncapped rendering. Original scheduling experiments remain off.

| Scene and framebuffer | OpenGL FPS | Vulkan FPS | FPS change | p99, OpenGL → Vulkan |
| --- | ---: | ---: | ---: | ---: |
| Survival spawn, 2560×1600, pair 1 | 102.65 | 115.71 | +12.72% | 13.24 → 11.29 ms |
| Survival spawn, 2560×1600, pair 2 | 100.73 | 115.55 | +14.72% | 14.58 → 12.04 ms |
| Fixed elevated view, 3456×2234, fresh-cache pair | 70.28 | 88.95 | +26.57% | 16.27 → 12.96 ms |
| Same view, common generated cache | 66.86 | 81.30 | +21.60% | 16.48 → 13.59 ms |

Initial runs warm for 60 seconds; the common-cache pair warms for 120 seconds.
All record 120 active seconds. CSV counts and
means are checked against JSON, with no lost/excluded samples, no captured
visual-setting changes and no abnormal sampled tick-manager state. Original
mod and fixture code match within each pair. Captured-setting differences are
the intended backend and driver. Spawn order is Vulkan/OpenGL/OpenGL/Vulkan;
the fresh Retina pair is OpenGL/Vulkan and the cached pair Vulkan/OpenGL.

The elevated scene uses a private spectator camera at a fixed position to
measure visible terrain without collisions. The spawn scene runs ordinary
survival and has slight position drift. Neither fixture is in a playable pack.
World simulation, weather and distant-cache generation still vary. Near terrain
and water were visually inspected, but skies and distant-cache coverage are not
pixel identical. The cached pair starts from independent copies of the same
generated world and Distant Horizons database; it still allows further LOD
generation and weather changes. These pairs do not establish a general gain.
Vulkan has a single 51.66/53.13 ms stall in the fresh/cached pair; OpenGL's
maximum is 22.12/25.48 ms respectively.
Movement, fresh terrain, reloads and longer thermal behavior remain necessary.

The supplied user recording is a separate route/world workload at 3456×2234;
these rows must not be treated as a matched comparison with its 84.27 FPS.
[Initial selected observations](../benchmarks/m2-pro-2026-10-04/initial-observations.json)
retain the first spawn pairs. [Fresh Retina](../benchmarks/m2-pro-2026-10-04/retina-initial-observations.json)
and [common-cache Retina observations](../benchmarks/m2-pro-2026-10-04/retina-cached-observations.json)
retain the two larger-framebuffer pairs. Raw files, logs, images and worlds remain in
gitignored `.local/`.

## Mac-specific playable profile

`m2-metal` requests the game's existing Vulkan/MoltenVK backend and preserves
the supplied 32 detailed chunks, 128-chunk medium LOD, simulation distance 12
and FOV 110. Vanilla clouds are off, matching the supplied capture; DH clouds
remain enabled. It shares the balanced terrain configuration and keeps original
controllers disabled. This is a separate local test profile; it does not
overwrite the existing balanced instance or implement a new Metal renderer.
The game retains its OpenGL fallback, so inspect the recorded actual backend.

```sh
python3 tools/build_mod.py
python3 tools/build_pack.py --profile m2-metal
python3 tools/build_pack.py --profile m2-metal-batched
```

Import the resulting `.mrpack` from `dist/` into Prism as a new instance. Keep
native Java 25 and maximum heap 6144 MiB. Use a copy of your world and compare
the actual framebuffer size and visual settings before judging the difference.

`m2-metal-batched` adds the cloud-rendering experiment to otherwise identical
quality settings. It is explicitly opt-in and requires Minecraft 26.3, DH 3.3.4,
Mac ARM, the actual Apple M2 Pro Vulkan device and no Iris. Unsupported devices
or backends use the original renderer. All other profiles leave the experiment
off. These 0.22.0 artifacts are local builds, not a published starter update.

The verified production-artifact pair at 3456×2234 averages 82.93 → 94.63 FPS
(+14.12%), with 1% lows 69.04 → 84.63 (+22.57%) and p99 13.03 → 10.93 ms
(-16.11%). Both warm and record for 120 seconds from the common copied cache;
candidate runs first, control second, with identical original JAR and fixture
hashes. Captured quality settings match, with no lost/excluded frames or abnormal
sampled simulation state. The candidate's maximum is 18.12 ms versus 52.34 ms
for this control; earlier prototype stalls remain part of the evidence.
[Production timing observations](../benchmarks/m2-pro-2026-10-04/cloud-batch-production-observations.json)
retain this pair. It is one final-artifact pair, separate from the two prototype
pairs and from the user's supplied route.

A separate 30-second flyover smoke travels about 326 horizontal blocks using
vanilla forward input at full Retina resolution, with batching active, normal
sampled simulation and no captured graphics changes. Its short run is a
functional check. Two clean 120-second flyover pairs measure 96.32 → 120.13
FPS (+24.72%) and, in reverse order, 95.43 → 119.62 FPS (+25.34%). p99
improves 14.29 → 12.68 ms and 14.40 → 12.74 ms. Each pair reaches exactly
matching end positions, runs at sampled normal 20 TPS and uses about 6.2 CPU
cores. Candidate worst frames are 52.20/59.15 ms versus control 47.46/64.62 ms;
isolated stalls persist. Asynchronous terrain completion, weather and hardware
clocks can still vary; these results do not establish a general-world speedup.
[Flyover observations](../benchmarks/m2-pro-2026-10-04/cloud-batch-flyover-observations.json)
retain both clean pairs. An earlier flyover attempt was discarded after
memory-heavy diagnostic analysis reached its recording interval.

## Profiling the next change

A separate 60-second Metal/JFR capture at 3456×2234 measures approximately
10.48 ms per GPU render interval and 0.62 ms per CPU `GameRenderer.render()`
call. Render-thread stack samples frequently include Vulkan submission waits.
These intervals cover different portions of the frame and sampling is not an
exact CPU/GPU utilization measurement. Profiling results are excluded from
ordinary FPS comparisons. OpenGL timestamps returned invalid zero intervals
on this driver; that profiling run was rejected rather than reporting a GPU win.

A later JFR diagnostic captures a 63.97 ms frame with no overlapping top-level
GC pause or recorded render-thread park/monitor wait. Three stack samples inside
that interval are in Vulkan native buffer creation, reached from DH terrain
vertex-buffer upload. This identifies a buffer-allocation candidate; one capture
does not establish a general cause or a fix. The first 43.90 ms diagnostic
interval has no stack samples and remains unattributed.
[Selected stall observations](../benchmarks/m2-pro-2026-10-04/stall-diagnostic-observations.json)
retain the correlation. `java tools/JfrSummary.java --frame-stalls recording.jfr`
uses two bounded streaming passes; overlap coverage avoids double counting
within each event category. GC and thread waits can overlap each other.

The optional private pass profiler samples every twentieth frame and brackets
the actual Vulkan render-pass operations with device timestamps. Pass timing
can disturb scheduling; it selects candidates rather than establishing a
performance gain. The initial profile points to the main world pass, distant
terrain geometry, TAA/sharpen and repeated generic rendering. SSAO is a smaller
cost in this view. Larger renderer or native-kernel changes should follow these
measurements and preserve the visible scene.

The common-cache pass capture has no missing or invalid queries across 254
sampled frames. The main pass averages 4.12 ms, the two distant-terrain passes
2.29 ms combined, and 38.25 generic-object passes per frame total 1.69 ms.
TAA/sharpen total 1.06 ms. Per-label frame totals include frames where that
label occurs; they are diagnostic intervals affected by timestamp insertion,
not percentages of total GPU utilization. The cloud batching prototype uses
separate transient uniform snapshots for each draw. Its matched Retina pairs
improve 83.71 → 96.46 FPS (+15.23%) and 84.11 → 96.62 FPS (+14.87%), with
opposite run order. p99 improves 12.98 → 10.72 ms and 12.90 → 10.65 ms.
Candidate worst frames are 51.69/65.59 ms versus control 37.10/43.90 ms;
the experiment has not solved isolated stalls.
[Cloud batching observations](../benchmarks/m2-pro-2026-10-04/cloud-batch-initial-observations.json)
retain both prototype pairs. A frozen daytime A/B/A check produces identical
RGB pixels across the sky/cloud region (rows 80–892), and at most one channel
level of near-terrain variation, including control/control variation.
[Image observations](../benchmarks/m2-pro-2026-10-04/cloud-batch-image-observations.json)
retain the readback hashes and differences. The original 0.22.0 production
artifact also passes a frozen A/B/A check: sky/cloud pixels are identical,
near terrain is identical in A/B, and three near-terrain pixels vary by one
channel level between the control images. Its 18,463 recorded batch draws
confirm that the production code was exercised.
[Production image observations](../benchmarks/m2-pro-2026-10-04/cloud-batch-production-image-observations.json)
retain that artifact's hash and differences. Distant-cache generation still
changes far pixels in both A/B and A/A; other weather, camera heights and
movement need tests. The batch closes before geometry uploads, callback
GPU commands and incompatible draws, preserving draw order. Each draw gets
its own transient uniform slice; no shared uniform is overwritten in flight.

```sh
python3 tools/mac_benchmark.py --profile m2-metal --backend vulkan \
  --label m2-local-test --seed-world /path/to/disposable/Demo_World \
  --scene overlook --fullscreen --warmup 120 --seconds 120
```

Use the same fixture JAR with `--fixture` for a matched pair. `--gpu-profile`
adds whole-render timestamps; `--pass-profile` adds sampled Vulkan pass timing.
`--jfr-profile` captures private JVM events and aligned frame intervals for
stall diagnosis. Its recording begins before the timed interval to warm the
profiler, and profiling results remain separate from ordinary FPS comparisons.
Keep profiling and ordinary timing labels separate. `--cloud-batch` tests the
private renderer prototype; `--cloud-image-check --scene overlook` with
`m2-metal-batched` validates the production renderer in a separate frozen A/B/A
image check without recording FPS. `--scene spawn` uses normal
survival; `overlook` changes only the private copied world to a fixed spectator
view. `--scene flyover` holds vanilla forward input during recording, exercising
normal spectator movement and terrain loading in that private copy. Existing
labels cannot be overwritten. No account data is needed.

## AFK measurement correction

Minecraft's default `afk` inactivity option caps stationary gameplay at 30 FPS
after one minute and 10 FPS after ten minutes, even when the configured cap is
unlimited. A focused stationary run measured 29.96 FPS and was rejected as a
renderer comparison. Private benchmarks select the vanilla `minimized` option;
playable profiles retain idle power saving.

The recorder now captures the inactivity option, effective limiter values and
reasons at the boundaries, and counts limiter/AFK observations during active
gameplay. Both comparison tools warn about these observations and mark different
inactivity options as mismatched. Legacy reports remain usable with an unknown
limiter warning. Disabling the idle cap is a measurement correction, not a game
performance optimization.

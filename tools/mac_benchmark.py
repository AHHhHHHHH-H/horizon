#!/usr/bin/env python3
"""Run an isolated physical-Mac benchmark using copied demo-world data."""

import argparse
import csv
import hashlib
import json
import math
import os
from pathlib import Path
import platform
import re
import shutil
import sys

try:
    from . import build_mod, build_pack
    from .test_culling_rays import run_fixture
except ImportError:
    import build_mod
    import build_pack
    from test_culling_rays import run_fixture


def validate_capture(result, instance, render, seconds):
    """Reject changed workloads and truncated files before interpreting throughput."""
    frames, ticks = result["frames"], result["ticks"]
    if frames.get("inactivity_fps_limit") != "minimized" or frames.get("frame_limiter_throttled_observations") != 0:
        raise AssertionError("The stationary workload was frame-throttled or limiter state was not captured")
    if frames["visual_settings_changed"]:
        raise AssertionError("Visual settings changed during measurement")
    if frames["render_distance"] != render or frames["simulation_distance"] != 12:
        raise AssertionError("Effective view or simulation settings differ")
    for kind, report in [("frames", frames), ("ticks", ticks)]:
        if report["kind"] != kind or report["dropped_samples"] or report["excluded_frames"]:
            raise AssertionError("Lost, excluded or misidentified measurement samples")
        if abs(report["elapsed_seconds"] - seconds) > 1:
            raise AssertionError("Measurement duration differs")
        if any(report[field] != result[field] for field in ["gpu", "graphics_backend"]):
            raise AssertionError("Captured device differs from the actual runtime device")
        sample_name = report["samples_file"]
        if Path(sample_name).name != sample_name:
            raise AssertionError("Invalid sample filename")
        with (instance / "horizon-benchmarks" / sample_name).open() as source:
            durations = [int(row["duration_ns"]) for row in csv.DictReader(source)]
        stats = report["stats"]
        if len(durations) != stats["count"] or len(durations) < 100 or min(durations) <= 0:
            raise AssertionError("CSV sample counts or durations are invalid")
        if not math.isclose(sum(durations) / len(durations) / 1e6, stats["meanMs"], rel_tol=1e-10):
            raise AssertionError("CSV timings differ from the summary")
        if kind == "frames" and abs(sum(durations) / 1e9 - report["elapsed_seconds"]) > 1:
            raise AssertionError("Active frame intervals do not cover the recording")
    if ticks["tick_state_changed"] or any(ticks["tick_state_" + field + "_samples"] for field in
            ["frozen", "stepping", "sprinting", "suppressed", "nonstandard_rate"]):
        raise AssertionError("Simulation tick-manager state changed or is nonstandard")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--profile", choices=build_pack.PROFILES, default="m2-balanced")
    parser.add_argument("--backend", choices=["opengl", "vulkan"], required=True)
    parser.add_argument("--label", required=True)
    parser.add_argument("--seed-world", type=Path, required=True)
    parser.add_argument("--warmup", type=int, default=60)
    parser.add_argument("--seconds", type=int, default=120)
    parser.add_argument("--render", type=int, default=32)
    parser.add_argument("--scene", choices=["spawn", "overlook", "flyover"], default="spawn",
                        help="Use survival spawn, a fixed spectator view, or a vanilla-input flyover")
    parser.add_argument("--width", type=int, default=1280)
    parser.add_argument("--height", type=int, default=800)
    parser.add_argument("--fullscreen", action="store_true")
    parser.add_argument("--heap", default="6G")
    parser.add_argument("--compact-headers", action="store_true")
    parser.add_argument("--cloud-image-check", action="store_true", help="Frozen-scene image validation only; no FPS measurement")
    parser.add_argument("--cloud-batch", action="store_true", help="Test private M2 Pro cloud batching; not part of playable packs")
    parser.add_argument("--gpu-profile", action="store_true", help="Add asynchronous GPU timings; keep separate from ordinary benchmarks")
    parser.add_argument("--jfr-profile", action="store_true", help="Capture JVM events and frame intervals separately from ordinary benchmarks")
    parser.add_argument("--pass-profile", action="store_true", help="Sample native Vulkan render-pass times separately from normal benchmarks")
    parser.add_argument("--fixture", type=Path, help="Reuse an earlier private fixture JAR for an identical-code comparison")
    parser.add_argument("--gc", choices=["default", "g1", "zgc"], default="default")
    args = parser.parse_args()
    if platform.system() != "Darwin" or platform.machine() not in {"arm64", "aarch64"}:
        parser.error("This benchmark targets the physical Apple Silicon Mac")
    if not re.fullmatch(r"[a-zA-Z0-9_-]{1,64}", args.label):
        parser.error("--label must contain 1 to 64 letters, digits, underscores or hyphens")
    if not (0 <= args.warmup <= 300 and 10 <= args.seconds <= 120 and 2 <= args.render <= 32):
        parser.error("Invalid warmup, measurement duration or view distance")
    production_cloud_batch = bool(build_pack.PROFILES[args.profile].get("cloud_batch"))
    if args.cloud_batch and production_cloud_batch:
        parser.error("Do not combine private and production cloud batch mixins")
    if args.cloud_image_check and (not (args.cloud_batch or production_cloud_batch) or args.scene != "overlook" or args.gpu_profile or args.pass_profile or args.jfr_profile):
        parser.error("--cloud-image-check requires cloud batching and --scene overlook, without profiling")
    if args.cloud_batch and (args.backend != "vulkan" or "distanthorizons" not in build_pack.PROFILES[args.profile]["mods"]):
        parser.error("--cloud-batch requires Vulkan and a Distant Horizons profile")
    if args.pass_profile and args.backend != "vulkan":
        parser.error("--pass-profile requires the Vulkan backend")
    if not (args.seed_world / "level.dat").is_file():
        parser.error("--seed-world must be an existing disposable demo-world directory")
    root = build_pack.ROOT
    instance = root / ".local/runtime/mac-benchmarks" / args.label
    if instance.exists():
        parser.error("This label already exists; use a new label to retain prior evidence")
    mixins = ["MacGpuTimingMixin"] if args.gpu_profile else []
    if args.jfr_profile:
        mixins.append("MacFrameIntervalMixin")
    if args.cloud_image_check and production_cloud_batch:
        mixins.append("MacDhCloudClockMixin")
    if args.cloud_batch:
        mixins.extend(["MacDhCloudBatchMixin", "MacDhCloudUniformMixin", "MacDhCloudBarrierMixin", "MacDhCloudClockMixin"])
    if args.pass_profile:
        mixins.extend(["MacGpuPassFrameMixin", "MacVulkanPassTimingMixin"])
    fixture = args.fixture or build_mod.build_integration_fixture("mac-benchmark",
        "dev.horizonlab.metrics.MacBenchmarkIntegration", fixture_dependencies={},
        fixture_mixins=mixins)
    mod = root / "dist" / f"horizon-lab-{build_pack.read_lock()['pack_version']}.jar"
    hashes = {"original_mod_sha256": hashlib.sha256(mod.read_bytes()).hexdigest(),
              "fixture_sha256": hashlib.sha256(fixture.read_bytes()).hexdigest()}
    build_pack.prepare_instance(args.profile, instance, mod)
    shutil.copytree(args.seed_world, instance / "saves/Demo_World")
    shutil.copy2(fixture, instance / "mods/mac-benchmark-fixture.jar")
    options = instance / "options.txt"
    text = options.read_text()
    text = re.sub(r"^renderDistance:.*$", f"renderDistance:{args.render}", text, flags=re.MULTILINE)
    text = re.sub(r"^preferredGraphicsBackend:.*$", "preferredGraphicsBackend:" + json.dumps(args.backend), text, flags=re.MULTILINE)
    # Match the supplied capture's expensive visual workload. Simulation remains 12.
    options.write_text(text + 'fov:1.0\nrenderClouds:"false"\ninactivityFpsLimit:"minimized"\n')
    (instance / "config/mac-benchmark.properties").write_text(
        f"label={args.label}\nwarmupSeconds={args.warmup}\nseconds={args.seconds}\n"
        f"gpuProfile={str(args.gpu_profile).lower()}\nscene={args.scene}\n"
        f"jfrProfile={str(args.jfr_profile).lower()}\n"
        f"passProfile={str(args.pass_profile).lower()}\ncloudBatch={str(args.cloud_batch).lower()}\n"
        f"cloudImageCheck={str(args.cloud_image_check).lower()}\n"
        f"productionCloudBatch={str(production_cloud_batch).lower()}\n")
    command = [sys.executable, str(root / "tools/runtime.py"), "demo", "--instance", str(instance),
        "--heap", args.heap, "--width", str(args.width), "--height", str(args.height), "--gc", args.gc]
    if args.compact_headers:
        command.append("--compact-headers")
    if args.fullscreen:
        command.append("--fullscreen")
    run = run_fixture(command, instance, os.environ.copy(), timeout=args.warmup + args.seconds + 180)
    result_file = instance / "mac-benchmark-result.json"
    if run.returncode or not result_file.is_file():
        raise RuntimeError(f"Mac benchmark failed; inspect the private log in {instance}")
    result = json.loads(result_file.read_text())
    if args.cloud_image_check:
        if not result.get("image_check_only") or not result.get("frozen_scene") or result.get("cloud_batch", {}).get("draws", 0) < 100:
            raise AssertionError("Frozen cloud image check was not exercised")
        if not all((instance / name).is_file() for name in ["cloud-image-control-a.png", "cloud-image-batched.png", "cloud-image-control-b.png"]):
            raise AssertionError("Cloud image readbacks are missing")
        result.update(hashes)
        result_file.write_text(json.dumps(result, indent=2) + "\n")
        print(json.dumps(result), flush=True)
        return
    frames = result["frames"]
    validate_capture(result, instance, args.render, args.seconds)
    if production_cloud_batch and (not frames.get("dh_cloud_batch_enabled") or frames.get("dh_cloud_batch_draws_recorded", 0) < 100):
        raise AssertionError("Production cloud batching was not active in the recording")
    if args.scene == "flyover" and math.hypot(result["player_x"] + 41.5, result["player_z"] - 72.5) < args.seconds:
        raise AssertionError("Private flyover did not move far enough to exercise terrain")
    if result.get("scene", "spawn") != args.scene:
        raise AssertionError("Fixture did not render the requested scene")
    batch = result.get("cloud_batch", {})
    if args.cloud_batch and (not batch.get("requested") or batch.get("draws", 0) < 100 or batch.get("passes", 0) < 1):
        raise AssertionError("Requested private cloud batch was not exercised")
    if not args.cloud_batch and batch.get("requested"):
        raise AssertionError("Unexpected private cloud batch")
    if args.gpu_profile:
        if result.get("gpu_profile", {}).get("gpu_render_duration", {}).get("count", 0) < 100:
            raise AssertionError("Requested GPU timings were not captured")
    elif "gpu_profile" in result:
        raise AssertionError("Unexpected GPU profiling in a timing-only run")
    if args.pass_profile:
        passes = result.get("gpu_pass_profile", {}).get("passes", {})
        if not passes or result["graphics_backend"] != "Vulkan":
            raise AssertionError("Requested native Vulkan pass timings were not captured")
    elif "gpu_pass_profile" in result:
        raise AssertionError("Unexpected GPU pass profiling in a timing-only run")
    if args.jfr_profile:
        if "jfr_profile" not in result or (instance / "mac-benchmark.jfr").stat().st_size < 1024:
            raise AssertionError("Requested JFR diagnostics were not captured")
    elif "jfr_profile" in result:
        raise AssertionError("Unexpected JFR profiling in a timing-only run")
    result["requested_backend"] = args.backend
    result["backend_fallback"] = result["graphics_backend"].lower() != args.backend
    result.update(hashes)
    result["profiling_enabled"] = args.gpu_profile or args.pass_profile or args.jfr_profile
    result_file.write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps({"label": args.label, "gpu": result["gpu"], "backend": result["graphics_backend"],
        "fallback": result["backend_fallback"], "frames": frames["stats"],
        "process_cpu_mean_cores": frames["process_cpu_mean_cores"],
        "resolution": [frames["framebuffer_width"], frames["framebuffer_height"]]}), flush=True)


if __name__ == "__main__":
    main()

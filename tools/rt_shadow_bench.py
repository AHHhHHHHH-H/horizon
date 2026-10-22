#!/usr/bin/env python3
"""Measures the EXPERIMENTAL ray-traced shadows (docs/raytracing.md) against the vanilla look.

For each render distance it starts the game twice on perf_bench's world (seed 1234, creative,
clear noon): once as players get it (no ray queries requested) and once with
HORIZON_RT_SHADOWS=1. Each start holds still at a few of perf_bench's scenes and records frame
and GPU times; the ray-traced start does it with the shadows drawn and with them off (the TLAS is
still rebuilt every frame), then rebuilds every section's BLAS to time the builds, and toggles a
box of blocks to time keeping the BLAS in sync with block updates. Screenshots go to --shots.

    python3 tools/rt_shadow_bench.py --build target/fast --distances 12,32 --out rt.json
"""
import argparse
import json
import os
import shutil
import statistics
import subprocess
import threading
import time
from pathlib import Path

import perf_bench as pb

SCENES = {name: (start, pitch) for name, start, _end, pitch in pb.SCENES}


def summary(frame_ms, gpu_ms):
    def pct(v, p):
        v = sorted(v)
        return v[min(len(v) - 1, int(len(v) * p))] if v else None
    return {
        "frames": len(frame_ms),
        "frame_ms_median": statistics.median(frame_ms) if frame_ms else None,
        "frame_ms_p99": pct(frame_ms, 0.99),
        "fps_avg": 1000 * len(frame_ms) / sum(frame_ms) if frame_ms else None,
        "gpu_ms_median": statistics.median(gpu_ms) if gpu_ms else None,
        "gpu_ms_p99": pct(gpu_ms, 0.99),
    }


def hold(control, start, pitch, seconds):
    x, y, z = start
    r = control(f"fly {x} {y} {z} {x} {y} {z} {seconds} {pitch} raw", timeout=seconds + 120)
    return summary(r.get("frame_ms", []), r.get("gpu_ms", []))


def memory(control, pid):
    renderer = control("status").get("renderer") or {}
    return {
        "gpu_allocated_mb": (renderer.get("gpu_allocated_bytes") or 0) / 2**20 or None,
        "nvidia_smi_mb": pb.nvidia_memory_mb(pid),
        "footprint_mb": (pb.phys_footprint(pid) or 0) / 2**20 or None,
        "resident_sections": renderer.get("resident_sections"),
    }


def rt_stats(control):
    return control("rt-shadows")["rt_shadows"]


def wait_built(control, limit=300):
    deadline = time.time() + limit
    while time.time() < deadline:
        s = rt_stats(control)
        if s["pending"] == 0:
            return s
        time.sleep(0.2)
    raise TimeoutError("BLAS builds still pending")


def during(control, start, pitch, seconds, action):
    """Frame figures of a hold while `action()` runs (from a second in)."""
    result = {}
    thread = threading.Thread(target=lambda: result.update(hold(control, start, pitch, seconds)))
    thread.start()
    time.sleep(1.0)
    extra = action()
    thread.join()
    return result, extra


def build_cost(control, start, pitch):
    """Every section's BLAS dropped and rebuilt (at most 512 a frame): GPU time per build, and the
    frames while it happens."""
    wait_built(control)
    before = len(rt_stats(control)["build_gpu_ms"])
    idle = rt_stats(control)["build_gpu_ms"][-60:]

    def rebuild():
        control("rt-shadows rebuild")
        time.sleep(0.5)
        return wait_built(control)
    frames, _ = during(control, start, pitch, 6, rebuild)
    time.sleep(1.0)
    s = rt_stats(control)
    pairs = list(zip(s["build_gpu_ms"], s["builds_per_frame"]))[-(len(s["build_gpu_ms"]) - before):]
    building = [(ms, n) for ms, n in pairs if n > 0]
    tlas_only = statistics.median(idle) if idle else 0.0
    built = sum(n for _, n in building)
    gpu = sum(ms for ms, _ in building) - tlas_only * len(building)
    return {
        "sections": s["blas_count"],
        "triangles": s["blas_triangles"],
        "tlas_only_gpu_ms": tlas_only,
        "build_frames": len(building),
        "blas_built_timed": built,
        "gpu_ms_per_blas": gpu / built if built else None,
        "gpu_us_per_1k_triangles": 1000 * gpu / (s["blas_triangles"] * built / max(s["blas_count"], 1) / 1000)
        if built and s["blas_triangles"] else None,
        "max_frame_build_gpu_ms": max((ms for ms, _ in building), default=None),
        "tlas_cpu_ms": s["cpu_ms"],
        "frames_during_rebuild": frames,
    }


def fills(control, start, rounds=20):
    """A 32-block cube near the player filled and emptied four times a second: its sections
    remesh (and, with the shadows, their BLAS are rebuilt)."""
    x, y, z = (int(v) for v in start)
    box = (x + 8, y - 40, z + 8, x + 39, y - 9, z + 39)
    for i in range(rounds):
        control("fill {} {} {} {} {} {} {}".format(*box, "stone" if i % 2 == 0 else "air"))
        time.sleep(0.25)


def block_updates(control, start, pitch, rounds=20):
    s0 = rt_stats(control)
    built0, n0 = s0["blas_built_total"], len(s0["build_gpu_ms"])
    t = time.time()
    frames, _ = during(control, start, pitch, rounds * 0.25 + 2, lambda: fills(control, start, rounds))
    time.sleep(1.0)
    s = rt_stats(control)
    ms = [m for m, n in zip(s["build_gpu_ms"], s["builds_per_frame"]) if n > 0][-(len(s["build_gpu_ms"]) - n0):]
    return {
        "rounds": rounds,
        "seconds": time.time() - t,
        "blas_rebuilt": s["blas_built_total"] - built0,
        "frames_with_builds_gpu_ms_median": statistics.median(ms) if ms else None,
        "frames_with_builds_gpu_ms_max": max(ms) if ms else None,
        "frames": frames,
    }


def run(args, distance, rt):
    label = f"rd{distance}-{'rt' if rt else 'vanilla'}"
    game_dir = pb.WORK / f"rt-{label}"
    shutil.rmtree(game_dir, ignore_errors=True)
    pb.write_options(game_dir, distance)
    sock = Path("/tmp") / f"horizon-rt-{os.getpid()}.sock"
    sock.unlink(missing_ok=True)
    command = [str(Path(args.build).resolve() / "horizon-client"), "--resources",
               str(Path(args.resources).resolve()), "--control-socket", str(sock), "--no-pause",
               "--weather", "clear", "--time", str(args.time)]
    if args.offscreen:
        command.append("--offscreen")
    env = dict(os.environ, HORIZON_NO_AUDIO="1", HORIZON_HOME=str(game_dir))
    env.pop("HORIZON_RT_SHADOWS", None)
    if rt:
        env["HORIZON_RT_SHADOWS"] = "1"
    log = (pb.WORK / f"rt-{label}.log").open("w")
    proc = subprocess.Popen(command, cwd=game_dir, env=env, stdout=log, stderr=subprocess.STDOUT)
    control = pb.Control(sock)
    out = {"distance": distance, "rt": rt, "scenes": {}}
    try:
        title, _ = pb.poll(control, lambda s: s.get("screen") == "title" and s.get("renderer"), 300)
        out["renderer"] = {k: title["renderer"].get(k) for k in ("gpu", "backend", "driver", "size")}
        control(f"open {pb.WORLD} {pb.SEED} creative")
        pb.wait_loaded(control, lambda p: True, 600)
        if rt:
            out["available"] = control("rt-shadows").get("rt_shadows") is not None
        for name in args.scenes.split(","):
            start, pitch = SCENES[name]
            control(f"time {args.time}")
            control("tp {} {} {}".format(*start))
            x, y, z = start
            control(f"fly {x} {y} {z} {x} {y} {z} 0.1 {pitch}")
            control(f"look {pb.yaw(start, pb.SCENES[[s[0] for s in pb.SCENES].index(name)][2]):.3f} {pitch}")
            pb.wait_loaded(control, lambda p: sum((p[i] - start[i]) ** 2 for i in range(3)) < 4, 600)
            time.sleep(2)
            scene = {}
            if rt and out.get("available"):
                wait_built(control)
                scene["rt_on"] = hold(control, start, pitch, args.seconds)
                scene["rt_on"].update(memory(control, proc.pid))
                scene["rt_stats"] = {k: v for k, v in rt_stats(control).items()
                                     if k not in ("build_gpu_ms", "builds_per_frame")}
                if args.shots:
                    control(f"screenshot {Path(args.shots).resolve() / f'{name}-{label}-on.png'}")
                control("rt-shadows off")
                time.sleep(1)
                scene["rt_off_tlas_kept"] = hold(control, start, pitch, args.seconds)
                if args.shots:
                    control(f"screenshot {Path(args.shots).resolve() / f'{name}-{label}-off.png'}")
                control("rt-shadows on")
                time.sleep(1)
                scene["build_cost"] = build_cost(control, start, pitch)
                scene["block_updates"] = block_updates(control, start, pitch)
            else:
                scene["vanilla"] = hold(control, start, pitch, args.seconds)
                scene["vanilla"].update(memory(control, proc.pid))
                if args.shots:
                    control(f"screenshot {Path(args.shots).resolve() / f'{name}-{label}.png'}")
                scene["block_updates"] = {"frames": during(control, start, pitch, 7, lambda: fills(control, start))[0]}
            out["scenes"][name] = scene
            print(label, name, json.dumps(scene), flush=True)
        try:
            control("quit", timeout=60)
        except (OSError, ValueError, RuntimeError):
            pass
    finally:
        pb.stop(proc)
        sock.unlink(missing_ok=True)
        log.close()
    return out


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--build", default="target/fast")
    p.add_argument("--resources", default=str(pb.lab.RESOURCES))
    p.add_argument("--distances", default="12,32")
    p.add_argument("--scenes", default="village,mountain")
    p.add_argument("--seconds", type=float, default=10)
    p.add_argument("--modes", default="vanilla,rt")
    p.add_argument("--offscreen", action="store_true")
    p.add_argument("--time", type=int, default=6000, help="time of day (6000: noon)")
    p.add_argument("--shots")
    p.add_argument("--out", required=True)
    args = p.parse_args()
    pb.WORK.mkdir(parents=True, exist_ok=True)
    if args.shots:
        Path(args.shots).mkdir(parents=True, exist_ok=True)
    results = []
    for distance in (int(d) for d in args.distances.split(",")):
        for mode in args.modes.split(","):
            results.append(run(args, distance, mode == "rt"))
            Path(args.out).write_text(json.dumps(results, indent=1))


if __name__ == "__main__":
    main()

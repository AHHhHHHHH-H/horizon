#!/usr/bin/env python3
"""Benchmark the real world behaviour-loader portion of client startup.

The level_shape_fixture example loads the registry and Behaviours::load before
reading cases. An empty case file excludes game rendering and world simulation.
For comparisons, build each revision separately, then pass the two binaries:

  python3 tools/profile_client_startup.py --resources /path/to/26.3 \
      --baseline-binary /baseline/fast/examples/level_shape_fixture \
      --optimized-binary /optimized/fast/examples/level_shape_fixture \
      --warmups 3 --runs 15 --output benchmarks/loader-comparison.json

Comparison runs alternate order to reduce cache/warmup bias; compilation and
hashing are never included in the timed section. This is NOT time-to-title.
"""

import argparse
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import platform
import resource
import statistics
import subprocess
import time


ROOT = Path(__file__).resolve().parent.parent
RESOURCE_FILES = ("facts.json", "block-behaviours.json", "gameplay-facts.json",
                  "block-tags.json", "cached-collision.nbt")


def digest(path):
    h = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            h.update(chunk)
    return h.hexdigest()


def summary(samples):
    ordered = sorted(samples)
    return {
        "samples_ms": [round(v, 3) for v in samples],
        "min_ms": round(ordered[0], 3),
        "p10_ms": round(ordered[int(0.1 * (len(ordered) - 1))], 3),
        "median_ms": round(statistics.median(ordered), 3),
        "p90_ms": round(ordered[int(0.9 * (len(ordered) - 1))], 3),
        "max_ms": round(ordered[-1], 3),
        "mean_ms": round(statistics.mean(samples), 3),
        "stdev_ms": round(statistics.stdev(samples), 3) if len(samples) > 1 else None,
    }


def cpu_pressure():
    """Linux CPU pressure counters; None where PSI is unavailable."""
    path = Path("/proc/pressure/cpu")
    if not path.exists():
        return None
    line = next((row for row in path.read_text().splitlines()
                 if row.startswith("some ")), None)
    return ({key: float(value) for key, value in
             (field.split("=", 1) for field in line.split()[1:])}
            if line else None)


def measure(binary, resources, affinity):
    before = resource.getrusage(resource.RUSAGE_CHILDREN)
    start = time.perf_counter_ns()
    subprocess.run([str(binary), str(resources), os.devnull, os.devnull],
                   check=True, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE,
                   preexec_fn=(lambda: os.sched_setaffinity(0, affinity)) if affinity else None)
    wall_ms = (time.perf_counter_ns() - start) / 1_000_000
    after = resource.getrusage(resource.RUSAGE_CHILDREN)
    return {
        "wall_ms": wall_ms,
        "cpu_ms": 1000 * (after.ru_utime + after.ru_stime - before.ru_utime - before.ru_stime),
        "minor_faults": after.ru_minflt - before.ru_minflt,
        "major_faults": after.ru_majflt - before.ru_majflt,
        "voluntary_switches": after.ru_nvcsw - before.ru_nvcsw,
        "involuntary_switches": after.ru_nivcsw - before.ru_nivcsw,
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__,
                                     formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--resources", type=Path, required=True)
    parser.add_argument("--runs", type=int, default=15)
    parser.add_argument("--warmups", type=int, default=3)
    parser.add_argument("--baseline-binary", type=Path)
    parser.add_argument("--optimized-binary", type=Path)
    parser.add_argument("--binary", type=Path, help="profile a previously built single binary")
    parser.add_argument("--affinity", help="Linux CPUs for each child, e.g. 2,3")
    parser.add_argument("--pause-ms", type=int, default=0,
                        help="sleep between measured pairs (outside timings)")
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    if args.runs < 1 or args.warmups < 0 or args.pause_ms < 0:
        parser.error("runs must be positive and warmups nonnegative")
    affinity = None
    if args.affinity:
        if not hasattr(os, "sched_setaffinity"):
            parser.error("CPU affinity requires Linux")
        try:
            affinity = {int(cpu) for cpu in args.affinity.split(",")}
        except ValueError:
            parser.error("affinity must be a comma-separated list of CPU IDs")
        if not affinity or not affinity.issubset(os.sched_getaffinity(0)):
            parser.error("affinity CPUs must be in the available CPU set")
    paired = args.baseline_binary is not None or args.optimized_binary is not None
    if paired and (not args.baseline_binary or not args.optimized_binary or args.binary):
        parser.error("paired mode requires both --baseline-binary and --optimized-binary")
    if args.binary and paired:
        parser.error("--binary is incompatible with paired mode")
    resources = args.resources.resolve(strict=True)
    for name in RESOURCE_FILES:
        if not (resources / name).is_file():
            parser.error(f"missing {resources / name}")

    if paired:
        binaries = {"baseline": args.baseline_binary, "optimized": args.optimized_binary}
    elif args.binary:
        binaries = {"loader": args.binary}
    else:
        env = os.environ.copy()
        target_dir = Path(env.get("CARGO_TARGET_DIR", ROOT / "target")).resolve()
        subprocess.run(
            ["cargo", "build", "--profile", "fast", "-p", "horizon-world",
             "--example", "level_shape_fixture"],
            cwd=ROOT, env=env, check=True,
        )
        binaries = {"loader": target_dir / "fast/examples/level_shape_fixture"}
    binaries = {name: path.resolve(strict=True) for name, path in binaries.items()}
    if any(not path.is_file() for path in binaries.values()):
        parser.error("all benchmark binaries must be regular files")

    result = {
        "scope": "registry plus world Behaviours::load, ending before fixture cases; not full client startup",
        "utc_start": datetime.now(timezone.utc).isoformat(),
        "platform": platform.platform(),
        "python": platform.python_version(),
        "cpu_model": next(
            (line.partition(":")[2].strip() for line in
             Path("/proc/cpuinfo").read_text().splitlines()
             if line.startswith("model name")), platform.processor()
        ) if Path("/proc/cpuinfo").exists() else platform.processor(),
        "available_cpus": sorted(os.sched_getaffinity(0)) if hasattr(os, "sched_getaffinity") else None,
        "resources": str(resources),
        "resource_sha256": {name: digest(resources / name) for name in RESOURCE_FILES},
        "profile": "fast",
        "binaries": {name: {"path": str(path), "sha256": digest(path)}
                     for name, path in binaries.items()},
        "warmups_per_binary": args.warmups,
        "samples_per_binary": args.runs,
        "comparison_order": "alternating baseline/optimized then optimized/baseline" if paired
                            else "single binary",
        "child_affinity": sorted(affinity) if affinity else None,
        "pause_between_pairs_ms": args.pause_ms,
        "measurement": "fresh process; wall clock and child user+system CPU; resource usage per child",
    }
    samples = {name: [] for name in binaries}
    load_averages = []
    cpu_samples = {name: [] for name in binaries}
    run_details = []
    pressure_samples = []
    labels = list(binaries)
    for i in range(args.warmups + args.runs):
        # First round baseline-first, second optimized-first; also interleave warmups.
        order = labels if i % 2 == 0 else labels[::-1]
        run = {}
        for name in order:
            data = measure(binaries[name], resources, affinity)
            if i >= args.warmups:
                samples[name].append(data["wall_ms"])
                cpu_samples[name].append(data["cpu_ms"])
                run[name] = {k: round(value, 3) for k, value in data.items()}
        if i >= args.warmups and hasattr(os, "getloadavg"):
            load_averages.append(round(os.getloadavg()[0], 2))
        if i >= args.warmups:
            run_details.append({"order": order, "measurements": run})
            pressure_samples.append(cpu_pressure())
        if args.pause_ms:
            time.sleep(args.pause_ms / 1000)
    result["results"] = {name: summary(values) for name, values in samples.items()}
    result["child_cpu_results"] = {name: summary(values) for name, values in cpu_samples.items()}
    result["per_pair_details"] = run_details
    result["system_load_1m_after_each_pair"] = load_averages
    result["cpu_pressure_after_each_pair"] = pressure_samples
    if paired:
        deltas = [after - before for before, after in
                  zip(samples["baseline"], samples["optimized"])]
        result["paired_after_minus_before_ms"] = summary(deltas)
        result["paired_cpu_after_minus_before_ms"] = summary([
            after - before for before, after in
            zip(cpu_samples["baseline"], cpu_samples["optimized"])
        ])
    result["utc_end"] = datetime.now(timezone.utc).isoformat()
    print(json.dumps(result, indent=2))
    if args.output:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(json.dumps(result, indent=2) + "\n")


if __name__ == "__main__":
    main()

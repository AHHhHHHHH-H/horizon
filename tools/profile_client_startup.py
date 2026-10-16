#!/usr/bin/env python3
"""Measure the world behaviour-loader phase of client startup.

The existing level_shape_fixture binary loads the real block registry and calls
Behaviours::load before reading cases. Supplying an empty case file measures
that same startup path without the game renderer, graphics driver or network.
This is a loader-phase measurement, not a full client time-to-title benchmark.
"""

import argparse
import json
import os
from pathlib import Path
import platform
import statistics
import subprocess
import time


ROOT = Path(__file__).resolve().parent.parent


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--resources", type=Path, required=True)
    parser.add_argument("--runs", type=int, default=7)
    parser.add_argument("--warmups", type=int, default=2)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    if args.runs < 1 or args.warmups < 0:
        parser.error("runs must be positive and warmups nonnegative")
    resources = args.resources.resolve(strict=True)
    for name in ("facts.json", "block-behaviours.json", "cached-collision.nbt"):
        if not (resources / name).is_file():
            parser.error(f"missing {resources / name}")

    env = os.environ.copy()
    target_dir = Path(env.get("CARGO_TARGET_DIR", ROOT / "target")).resolve()
    subprocess.run(
        ["cargo", "build", "--profile", "fast", "-p", "horizon-world",
         "--example", "level_shape_fixture"],
        cwd=ROOT, env=env, check=True,
    )
    binary = target_dir / "fast" / "examples" / "level_shape_fixture"
    command = [str(binary), str(resources), os.devnull, os.devnull]
    samples = []
    for i in range(args.warmups + args.runs):
        start = time.perf_counter()
        subprocess.run(command, cwd=ROOT, check=True, capture_output=True)
        elapsed = (time.perf_counter() - start) * 1000
        if i >= args.warmups:
            samples.append(round(elapsed, 3))
    result = {
        "scope": "world behaviour-loader process startup to completed Behaviours::load",
        "commit": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip(),
        "platform": platform.platform(),
        "resources": str(resources),
        "profile": "fast",
        "warmups": args.warmups,
        "runs": args.runs,
        "samples_ms": samples,
        "median_ms": round(statistics.median(samples), 3),
        "min_ms": min(samples),
        "max_ms": max(samples),
    }
    print(json.dumps(result, indent=2))
    if args.output:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(json.dumps(result, indent=2) + "\n")


if __name__ == "__main__":
    main()

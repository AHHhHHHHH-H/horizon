#!/usr/bin/env python3
"""Verify the streaming JFR reader against the JDK's own JSON exporter."""

import argparse
import json
import math
from pathlib import Path
import re
import subprocess

ROOT = Path(__file__).resolve().parents[1]
BUILD = ROOT / ".local/jfr-tests"


def main(collector="default"):
    BUILD.mkdir(parents=True, exist_ok=True)
    fixture = BUILD / "fixture.jfr"
    gc_flags = ["-XX:+UseZGC"] if collector == "zgc" else []
    classes = BUILD / "classes"
    classes.mkdir(exist_ok=True)
    subprocess.run(["javac", "--release", "25", "-d", str(classes),
                    str(ROOT / "tools/JfrSummary.java"), str(ROOT / "tests/java/JfrFixture.java")], check=True)
    subprocess.run(["java", "-Xmx128M", *gc_flags, "-cp", str(classes), "JfrFixture", str(fixture)], check=True)
    summary = json.loads(subprocess.check_output(
        ["java", str(ROOT / "tools/JfrSummary.java"), str(fixture)], text=True))
    events = json.loads(subprocess.check_output(
        ["jfr", "print", "--json", "--stack-depth", "1", "--events",
         "jdk.ThreadAllocationStatistics,jdk.GarbageCollection,jdk.GCPhasePause,jdk.GCHeapSummary,jdk.ExecutionSample,jdk.NativeMethodSample,jdk.ObjectAllocationSample", str(fixture)],
        text=True))["recording"]["events"]
    counters = {}
    for event in events:
        values = event["values"]
        if event["type"] == "jdk.ThreadAllocationStatistics" and values["thread"]:
            thread = values["thread"]
            key = thread["javaThreadId"]
            counters.setdefault(key, []).append((values["startTime"], values["allocated"]))
    for row in summary["allocation_threads"]:
        samples = sorted(counters[row["thread_id"]])
        assert row["counter_observations"] == len(samples)
        if len(samples) > 1 and samples[0][0] != samples[-1][0]:
            assert row["observed_allocated_bytes"] == samples[-1][1] - samples[0][1]
            assert row["observed_seconds"] > 0
        else:
            assert row["observed_allocated_bytes"] is None
    fixture_row = next(row for row in summary["allocation_threads"] if row["thread"].startswith("Horizon JFR fixture"))
    assert '"quoted"\tthread' in fixture_row["thread"]
    assert fixture_row["observed_allocated_bytes"] > 1_000_000
    assert summary["gc_cycles"] == sum(e["type"] == "jdk.GarbageCollection" for e in events)
    assert summary["gc_pause_events"] == sum(e["type"] == "jdk.GCPhasePause" for e in events)
    assert summary["heap_used_peak_at_gc_bytes"] == max(
        e["values"]["heapUsed"] for e in events if e["type"] == "jdk.GCHeapSummary")
    for key, types in [("execution_threads", {"jdk.ExecutionSample", "jdk.NativeMethodSample"}),
                       ("allocation_sample_threads", {"jdk.ObjectAllocationSample"})]:
        expected = {}
        for event in events:
            if event["type"] not in types:
                continue
            field = "eventThread" if event["type"] == "jdk.ObjectAllocationSample" else "sampledThread"
            thread = event["values"][field]
            if thread:
                expected[thread["javaThreadId"]] = expected.get(thread["javaThreadId"], 0) + 1
        assert {row["thread_id"]: row["samples"] for row in summary[key]} == expected
    def no_weight_keys(value):
        if isinstance(value, dict):
            assert "weight" not in value and "weighted_bytes" not in value
            for child in value.values():
                no_weight_keys(child)
        elif isinstance(value, list):
            for child in value:
                no_weight_keys(child)
    no_weight_keys(summary)
    frames = json.loads(subprocess.check_output(
        ["java", "-cp", str(classes), "JfrSummary", "--frame-stalls", str(fixture)], text=True))
    assert frames["frame_events"] == 1 and len(frames["slow_frames"]) == 1
    assert 0 < frames["slow_frames"][0]["gc_pause_overlap_ms"] < frames["slow_frames"][0]["interval_ms"]
    uploads = json.loads(subprocess.check_output(
        ["jfr", "print", "--json", "--events", "dev.horizonlab.TerrainUpload", str(fixture)],
        text=True))["recording"]["events"]
    assert len(uploads) == 2
    durations = [float(re.fullmatch(r"PT([0-9.]+)S", event["values"]["duration"]).group(1)) * 1000
                 for event in uploads]
    assert math.isclose(frames["slow_frames"][0]["terrain_upload_overlap_ms"], max(durations), abs_tol=1e-6)
    loads = frames["slow_frames"][0]["render_thread_class_loads"]
    payload = next(row for row in loads if row["class_name"] == "JfrFixture$LoadPayload")
    assert payload["duration_ms"] >= 5
    coverage = frames["slow_frames"][0]["render_thread_class_load_overlap_ms"]
    assert payload["overlap_ms"] <= coverage <= frames["slow_frames"][0]["interval_ms"]
    print("Streaming JFR summary matches real JDK allocation counters, stack-sample counts, GC event counts, heap observations and escaped thread names.")
    print("Frame/GC overlap, nested upload coverage, real slow class loading and 10,000 seeded interval-union cases checked.")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--gc", choices=["default", "zgc"], default="default")
    main(parser.parse_args().gc)

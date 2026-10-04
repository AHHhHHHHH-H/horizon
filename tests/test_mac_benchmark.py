import copy
import csv
from pathlib import Path
import tempfile
import unittest

from tools.mac_benchmark import validate_capture


class MacCaptureTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.instance = Path(self.directory.name)
        output = self.instance / "horizon-benchmarks"
        output.mkdir()
        self.result = {"gpu": "Apple M2 Pro", "graphics_backend": "Vulkan"}
        for kind, duration in [("frames", 120_000_000), ("ticks", 5_000_000)]:
            filename = kind + ".csv"
            with (output / filename).open("w") as stream:
                writer = csv.writer(stream)
                writer.writerow(["sample", "duration_ns"])
                writer.writerows((index, duration) for index in range(1000))
            self.result[kind] = {"kind": kind, "gpu": "Apple M2 Pro", "graphics_backend": "Vulkan",
                "elapsed_seconds": 120, "dropped_samples": 0, "excluded_frames": 0,
                "samples_file": filename, "stats": {"count": 1000, "meanMs": duration / 1e6}}
        self.result["frames"].update(visual_settings_changed=[], render_distance=32, simulation_distance=12,
            inactivity_fps_limit="minimized", frame_limiter_throttled_observations=0)
        self.result["ticks"].update(tick_state_changed=[])
        for field in ["frozen", "stepping", "sprinting", "suppressed", "nonstandard_rate"]:
            self.result["ticks"]["tick_state_" + field + "_samples"] = 0

    def test_complete_active_capture_is_accepted(self):
        validate_capture(self.result, self.instance, 32, 120)

    def test_idle_focused_capture_and_missing_limiter_measurements_are_rejected(self):
        for value in [None, 1, 3600]:
            report = copy.deepcopy(self.result)
            report["frames"]["frame_limiter_throttled_observations"] = value
            with self.subTest(value=value), self.assertRaises(AssertionError):
                validate_capture(report, self.instance, 32, 120)

    def test_incomplete_files_changed_graphics_and_nonstandard_simulation_are_rejected(self):
        changes = [("frames", "visual_settings_changed", ["render_distance"]),
            ("frames", "excluded_frames", 1), ("frames", "samples_file", "../private.csv"),
            ("frames", "graphics_backend", "OpenGL"), ("ticks", "tick_state_frozen_samples", 1),
            ("ticks", "dropped_samples", 1), ("ticks", "elapsed_seconds", 60)]
        for kind, key, value in changes:
            report = copy.deepcopy(self.result)
            report[kind][key] = value
            with self.subTest(kind=kind, key=key), self.assertRaises(AssertionError):
                validate_capture(report, self.instance, 32, 120)
        report = copy.deepcopy(self.result)
        report["frames"]["stats"]["count"] = 999
        with self.assertRaises(AssertionError):
            validate_capture(report, self.instance, 32, 120)


if __name__ == "__main__":
    unittest.main()

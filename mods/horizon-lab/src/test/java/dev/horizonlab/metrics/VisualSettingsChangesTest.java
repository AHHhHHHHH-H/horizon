package dev.horizonlab.metrics;

import java.util.LinkedHashMap;
import java.util.Map;

public final class VisualSettingsChangesTest {
    public static void main(String[] args) {
        var visual = Map.<String, Object>of("render_distance", 32, "dh_runtime_ratio", 0.5,
            "dh_cloud_batch_enabled", true, "iris_capture_status", "not_installed");
        var snapshot = new LinkedHashMap<String, Object>(visual);
        if (!VisualSettingsChanges.between(snapshot, visual).isEmpty())
            throw new AssertionError("Identical visual settings were treated as changed");
        var end = new LinkedHashMap<String, Object>(visual);
        end.put("dh_runtime_ratio", 0.25);
        if (!VisualSettingsChanges.between(snapshot, end).isEmpty())
            throw new AssertionError("Expected adaptive duty-cycle change was treated as visual quality");
        end.put("dh_cloud_batch_enabled", false);
        end.put("render_distance", 12);
        end.remove("iris_capture_status");
        var changed = VisualSettingsChanges.between(snapshot, end);
        if (!changed.containsAll(java.util.List.of("dh_cloud_batch_enabled", "render_distance", "iris_capture_status"))
                || changed.size() != 3) throw new AssertionError("Actual visual changes were missed: " + changed);
        System.out.println("Visual snapshot equality, adaptive duty cycle and actual changes checked.");
    }
}

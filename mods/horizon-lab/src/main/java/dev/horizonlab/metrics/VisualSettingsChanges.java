package dev.horizonlab.metrics;

import java.util.ArrayList;
import java.util.List;
import java.util.Map;
import java.util.Objects;

/** Compare visual snapshots independently of changing measurement metadata. */
public final class VisualSettingsChanges {
    private VisualSettingsChanges() {}

    public static List<String> between(Map<String, ?> before, Map<String, ?> after) {
        var changed = new ArrayList<String>();
        for (var setting : after.entrySet()) {
            if (!setting.getKey().equals("dh_runtime_ratio")
                    && !Objects.equals(before.get(setting.getKey()), setting.getValue()))
                changed.add(setting.getKey());
        }
        for (String setting : before.keySet()) {
            if ((setting.startsWith("dh_") || setting.startsWith("iris_")) && !after.containsKey(setting))
                changed.add(setting);
        }
        return changed;
    }
}

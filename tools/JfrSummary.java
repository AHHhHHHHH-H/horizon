import java.nio.file.Path;
import java.time.Duration;
import java.time.Instant;
import java.util.ArrayList;
import java.util.Comparator;
import java.util.HashMap;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import jdk.jfr.consumer.RecordedEvent;
import jdk.jfr.consumer.RecordedThread;
import jdk.jfr.consumer.RecordingFile;

/** Streams a private JFR without expanding class loaders or exporting command-line arguments. */
public final class JfrSummary {
    static final class Allocation {
        String name;
        Instant firstTime, lastTime;
        long firstBytes, lastBytes, observations;
        boolean invalid;
        void add(Instant time, long bytes, String label) {
            name = label;
            observations++;
            if (bytes < 0) invalid = true;
            if (firstTime == null || time.isBefore(firstTime)) { firstTime = time; firstBytes = bytes; }
            if (lastTime == null || time.isAfter(lastTime)) { lastTime = time; lastBytes = bytes; }
            if (firstTime.equals(lastTime) && firstBytes != lastBytes) invalid = true;
        }
        Map<String, Object> result(long id) {
            var row = new LinkedHashMap<String, Object>();
            row.put("thread_id", id);
            row.put("thread", name);
            row.put("counter_observations", observations);
            double span = Duration.between(firstTime, lastTime).toNanos() / 1e9;
            row.put("observed_seconds", span);
            boolean valid = !invalid && observations > 1 && span > 0 && lastBytes >= firstBytes;
            row.put("observed_allocated_bytes", valid ? lastBytes - firstBytes : null);
            row.put("observed_bytes_per_second", valid ? (lastBytes - firstBytes) / span : null);
            return row;
        }
    }

    static final class Samples {
        String name;
        long count, missingStacks, truncatedStacks;
        final Map<String, Long> leaf = new HashMap<>(), inclusive = new HashMap<>();
        void add(RecordedEvent event, String label) {
            name = label;
            count++;
            var stack = event.getStackTrace();
            if (stack == null || stack.getFrames().isEmpty()) { missingStacks++; return; }
            if (stack.isTruncated()) truncatedStacks++;
            var methods = new java.util.HashSet<String>();
            for (var frame : stack.getFrames()) {
                var method = frame.getMethod();
                methods.add(method.getType().getName() + "." + method.getName() + method.getDescriptor());
            }
            for (String method : methods) inclusive.merge(method, 1L, Long::sum);
            var method = stack.getFrames().getFirst().getMethod();
            leaf.merge(method.getType().getName() + "." + method.getName() + method.getDescriptor(), 1L, Long::sum);
        }
        Map<String, Object> result(long id) {
            return Map.of("thread_id", id, "thread", name, "samples", count,
                "missing_stacks", missingStacks, "truncated_stacks", truncatedStacks,
                "top_leaf_methods", top(leaf), "top_inclusive_methods", top(inclusive));
        }
    }

    static long threadId(RecordedThread thread) {
        return thread.getJavaThreadId() > 0 ? thread.getJavaThreadId() : -thread.getId();
    }

    static String threadName(RecordedThread thread) {
        if (thread.getJavaName() != null) return thread.getJavaName();
        return thread.getOSName() == null ? "unnamed thread" : thread.getOSName();
    }

    static List<Map<String, Object>> top(Map<String, Long> counts) {
        return counts.entrySet().stream()
            .sorted(Map.Entry.<String, Long>comparingByValue().reversed().thenComparing(Map.Entry.comparingByKey()))
            .limit(15).map(e -> Map.<String, Object>of("method", e.getKey(), "samples", e.getValue())).toList();
    }

    static Map<String, Object> summarize(Path path) throws Exception {
        Map<Long, Allocation> allocations = new HashMap<>();
        Map<Long, Samples> execution = new HashMap<>(), allocationSamples = new HashMap<>();
        long events = 0, gcCycles = 0, pauseCount = 0, heapPeak = 0;
        double pauseMs = 0, longestPauseMs = 0;
        Instant first = null, last = null;
        try (var recording = new RecordingFile(path)) {
            while (recording.hasMoreEvents()) {
                var event = recording.readEvent();
                events++;
                Instant time = event.getStartTime();
                if (first == null || time.isBefore(first)) first = time;
                Instant end = event.getEndTime();
                if (last == null || end.isAfter(last)) last = end;
                switch (event.getEventType().getName()) {
                    case "jdk.ThreadAllocationStatistics" -> {
                        RecordedThread thread = event.getThread("thread");
                        if (thread != null) allocations.computeIfAbsent(threadId(thread), k -> new Allocation())
                            .add(time, event.getLong("allocated"), threadName(thread));
                    }
                    case "jdk.ExecutionSample", "jdk.NativeMethodSample", "jdk.ObjectAllocationSample" -> {
                        boolean allocation = event.getEventType().getName().equals("jdk.ObjectAllocationSample");
                        RecordedThread thread = event.getThread(allocation ? "eventThread" : "sampledThread");
                        if (thread != null) (allocation ? allocationSamples : execution)
                            .computeIfAbsent(threadId(thread), k -> new Samples()).add(event, threadName(thread));
                    }
                    case "jdk.GarbageCollection" -> gcCycles++;
                    case "jdk.GCPhasePause" -> {
                        double ms = event.getDuration().toNanos() / 1e6;
                        pauseMs += ms;
                        longestPauseMs = Math.max(longestPauseMs, ms);
                        pauseCount++;
                    }
                    case "jdk.GCHeapSummary" -> heapPeak = Math.max(heapPeak, event.getLong("heapUsed"));
                    default -> { }
                }
            }
        }
        List<Map<String, Object>> allocationRows = new ArrayList<>();
        allocations.forEach((id, allocation) -> allocationRows.add(allocation.result(id)));
        allocationRows.sort(Comparator.<Map<String, Object>>comparingLong(r ->
            r.get("observed_allocated_bytes") instanceof Long bytes ? bytes : -1).reversed()
            .thenComparingLong(r -> (Long) r.get("thread_id")));
        var result = new LinkedHashMap<String, Object>();
        result.put("schema", 1);
        result.put("events_read", events);
        result.put("event_span_seconds", first == null ? 0 : Duration.between(first, last).toNanos() / 1e9);
        result.put("allocation_threads", allocationRows);
        result.put("execution_threads", sampleRows(execution));
        result.put("allocation_sample_threads", sampleRows(allocationSamples));
        result.put("gc_cycles", gcCycles);
        result.put("gc_pause_events", pauseCount);
        result.put("gc_pause_total_ms", pauseMs);
        result.put("gc_pause_longest_ms", longestPauseMs);
        result.put("heap_used_peak_at_gc_bytes", heapPeak == 0 ? null : heapPeak);
        result.put("notes", List.of(
            "Allocation bytes are differences between directly observed per-thread counters, with separate observation spans. Thread startup/exit gaps are not covered; missing intervals are null.",
            "Execution/native-method samples locate sampled stacks, not exact CPU time, GPU work or elapsed-time fractions. Inclusive methods count each sample once even during recursion.",
            "Allocation stack samples are counts only. Sample weights are deliberately excluded; counts are not allocated bytes or retained RAM.",
            "GC pause fields use top-level GCPhasePause events, excluding concurrent cycle duration and nested pause phases. GC heap peaks can miss between-GC peaks.",
            "Profiling changes workload overhead. Keep raw recordings private; they may contain environment details beyond this selected summary."));
        return result;
    }

    static List<Map<String, Object>> sampleRows(Map<Long, Samples> samples) {
        return samples.entrySet().stream().sorted(Comparator.<Map.Entry<Long, Samples>>comparingLong(e -> e.getValue().count)
            .reversed().thenComparingLong(Map.Entry::getKey)).map(e -> e.getValue().result(e.getKey())).toList();
    }

    record Span(long start, long end) {}

    static long coveredNs(List<Span> spans) {
        var ordered = spans.stream().filter(s -> s.end > s.start)
            .sorted(Comparator.comparingLong(Span::start)).toList();
        long total = 0, start = 0, end = 0;
        for (Span span : ordered) {
            if (span.start > end) {
                total += end - start;
                start = span.start;
            }
            end = Math.max(end, span.end);
        }
        return total + end - start;
    }

    static final class Stall {
        final Instant start, end;
        final long intervalNs;
        final List<Span> pauses = new ArrayList<>(), waits = new ArrayList<>();
        final Map<String, Long> samples = new HashMap<>();
        final Map<String, Long> inclusiveSamples = new HashMap<>();

        Stall(RecordedEvent event) {
            start = event.getStartTime(); end = event.getEndTime();
            intervalNs = event.getLong("intervalNs");
        }

        void overlap(RecordedEvent event, List<Span> spans) {
            Instant first = event.getStartTime().isAfter(start) ? event.getStartTime() : start;
            Instant last = event.getEndTime().isBefore(end) ? event.getEndTime() : end;
            if (!first.isBefore(last)) return;
            if (spans.size() >= 4096) throw new IllegalArgumentException("Too many overlapping diagnostic events");
            spans.add(new Span(Duration.between(start, first).toNanos(), Duration.between(start, last).toNanos()));
        }

        Map<String, Object> result() {
            return Map.of("start", start.toString(), "interval_ms", intervalNs / 1e6,
                "gc_pause_overlap_ms", coveredNs(pauses) / 1e6,
                "render_thread_wait_overlap_ms", coveredNs(waits) / 1e6,
                "render_thread_sample_leaf_methods", top(samples),
                "render_thread_sample_inclusive_methods", top(inclusiveSamples));
        }
    }

    /** Two bounded streaming passes, without expanding JFR class-loader graphs. */
    static Map<String, Object> frameStalls(Path path) throws Exception {
        var stalls = new ArrayList<Stall>();
        long frames = 0;
        try (var file = new RecordingFile(path)) {
            while (file.hasMoreEvents()) {
                var event = file.readEvent();
                String type = event.getEventType().getName();
                if (!type.equals("dev.horizonlab.FrameInterval")
                        && !type.equals("dev.horizonlab.metrics.MacBenchmarkIntegration$FrameIntervalEvent")) continue;
                frames++;
                if (event.getLong("intervalNs") < 25_000_000) continue;
                if (stalls.size() >= 4096) throw new IllegalArgumentException("Too many slow frames for bounded analysis");
                stalls.add(new Stall(event));
            }
        }
        if (frames == 0) throw new IllegalArgumentException("No private frame-interval events captured");
        try (var file = new RecordingFile(path)) {
            while (file.hasMoreEvents()) {
                var event = file.readEvent();
                String type = event.getEventType().getName();
                boolean pause = type.equals("jdk.GCPhasePause");
                boolean wait = type.equals("jdk.ThreadPark") || type.equals("jdk.JavaMonitorEnter")
                    || type.equals("jdk.JavaMonitorWait");
                boolean sample = type.equals("jdk.ExecutionSample") || type.equals("jdk.NativeMethodSample");
                if (!pause && !wait && !sample) continue;
                if (!pause) {
                    var thread = event.getThread(sample ? "sampledThread" : "eventThread");
                    if (thread == null || !threadName(thread).equals("Render thread")) continue;
                }
                for (Stall stall : stalls) {
                    if (pause) stall.overlap(event, stall.pauses);
                    else if (wait) stall.overlap(event, stall.waits);
                    else if (!event.getStartTime().isBefore(stall.start) && event.getStartTime().isBefore(stall.end)) {
                        var stack = event.getStackTrace();
                        if (stack != null && !stack.getFrames().isEmpty()) {
                            var method = stack.getFrames().getFirst().getMethod();
                            stall.samples.merge(method.getType().getName() + "." + method.getName(), 1L, Long::sum);
                            var methods = new java.util.HashSet<String>();
                            for (var frame : stack.getFrames()) {
                                var inclusive = frame.getMethod();
                                methods.add(inclusive.getType().getName() + "." + inclusive.getName());
                            }
                            for (String inclusive : methods) stall.inclusiveSamples.merge(inclusive, 1L, Long::sum);
                        }
                    }
                }
            }
        }
        return Map.of("frame_events", frames, "slow_frame_threshold_ms", 25,
            "slow_frames", stalls.stream().sorted(Comparator.comparing(s -> s.start)).map(Stall::result).toList(),
            "note", "Diagnostic overlap is not causation. GC and thread waits can overlap each other; sampled stacks can miss short work. Profiling intervals are not ordinary FPS measurements.");
    }

    static String json(Object value) {
        if (value == null) return "null";
        if (value instanceof String string) {
            var out = new StringBuilder("\"");
            for (int i = 0; i < string.length(); i++) {
                char c = string.charAt(i);
                if (c == '"' || c == '\\') out.append('\\').append(c);
                else if (c < 32) out.append(String.format(java.util.Locale.ROOT, "\\u%04x", (int) c));
                else out.append(c);
            }
            return out.append('"').toString();
        }
        if (value instanceof Number || value instanceof Boolean) return value.toString();
        if (value instanceof Map<?, ?> map) return map.entrySet().stream()
            .map(e -> json(e.getKey().toString()) + ":" + json(e.getValue()))
            .collect(java.util.stream.Collectors.joining(",", "{", "}"));
        if (value instanceof List<?> list) return list.stream().map(JfrSummary::json)
            .collect(java.util.stream.Collectors.joining(",", "[", "]"));
        throw new IllegalArgumentException("Unsupported JSON value");
    }

    public static void main(String[] args) throws Exception {
        if (args.length == 2 && args[0].equals("--frame-stalls")) {
            System.out.println(json(frameStalls(Path.of(args[1]))));
            return;
        }
        if (args.length != 1) throw new IllegalArgumentException("Usage: java tools/JfrSummary.java private-recording.jfr");
        System.out.println(json(summarize(Path.of(args[0]))));
    }
}

import java.nio.file.Path;
import java.time.Duration;
import jdk.jfr.Recording;

/** Produces real JDK counter/GC events for the reader's independent integration check. */
public final class JfrFixture {
    private static volatile Object[] retained;
    @jdk.jfr.Name("dev.horizonlab.FrameInterval")
    @jdk.jfr.StackTrace(false)
    public static final class FrameInterval extends jdk.jfr.Event {
        public long intervalNs;
    }
    @jdk.jfr.Name("dev.horizonlab.TerrainUpload")
    @jdk.jfr.StackTrace(false)
    public static final class TerrainUpload extends jdk.jfr.Event {}
    public static final class LoadPayload {}

    static void checkOverlapCoverage() {
        var random = new java.util.Random(0x4d3250524fL);
        for (int test = 0; test < 10_000; test++) {
            var spans = new java.util.ArrayList<JfrSummary.Span>();
            var occupied = new boolean[128];
            for (int i = random.nextInt(16); i > 0; i--) {
                int start = random.nextInt(128), end = start + random.nextInt(129 - start);
                spans.add(new JfrSummary.Span(start, end));
                java.util.Arrays.fill(occupied, start, end, true);
            }
            long expected = 0;
            for (boolean present : occupied) if (present) expected++;
            if (JfrSummary.coveredNs(spans) != expected)
                throw new AssertionError("Overlapping event intervals were double-counted");
            java.util.Collections.shuffle(spans, random);
            if (JfrSummary.coveredNs(spans) != expected)
                throw new AssertionError("Coverage depends on event order");
        }
    }
    public static void main(String[] args) throws Exception {
        checkOverlapCoverage();
        Thread.currentThread().setName("Horizon JFR fixture \"quoted\"\tthread");
        try (var recording = new Recording()) {
            recording.enable("jdk.ThreadAllocationStatistics").withPeriod(Duration.ofMillis(100));
            recording.enable("jdk.ExecutionSample").withPeriod(Duration.ofMillis(10));
            recording.enable("jdk.ObjectAllocationSample").withStackTrace();
            recording.enable("jdk.GarbageCollection");
            recording.enable("jdk.GCPhasePause");
            recording.enable("jdk.GCHeapSummary");
            recording.enable(FrameInterval.class).withThreshold(Duration.ZERO);
            recording.enable(TerrainUpload.class).withThreshold(Duration.ZERO);
            recording.enable("jdk.ClassLoad").withThreshold(Duration.ZERO);
            recording.start();
            for (int batch = 0; batch < 12; batch++) {
                Object[] objects = new Object[100_000];
                for (int i = 0; i < objects.length; i++) objects[i] = new byte[32 + (i % 8)];
                retained = objects;
                Thread.sleep(120);
            }
            var renderer = new Thread(() -> {
                var frame = new FrameInterval();
                long started = System.nanoTime();
                frame.begin();
                System.gc();
                try (var input = JfrFixture.class.getResourceAsStream("JfrFixture$LoadPayload.class")) {
                    byte[] bytes = input.readAllBytes();
                    var slowLoader = new ClassLoader(null) {
                        @Override protected Class<?> findClass(String name) throws ClassNotFoundException {
                            if (!name.equals("JfrFixture$LoadPayload")) throw new ClassNotFoundException(name);
                            java.util.concurrent.locks.LockSupport.parkNanos(10_000_000);
                            return defineClass(name, bytes, 0, bytes.length);
                        }
                    };
                    Class.forName("JfrFixture$LoadPayload", false, slowLoader);
                } catch (Exception failure) { throw new AssertionError(failure); }
                var outer = new TerrainUpload();
                outer.begin();
                var inner = new TerrainUpload();
                inner.begin();
                java.util.concurrent.locks.LockSupport.parkNanos(20_000_000);
                inner.end(); inner.commit();
                java.util.concurrent.locks.LockSupport.parkNanos(20_000_000);
                outer.end(); outer.commit();
                frame.end();
                frame.intervalNs = System.nanoTime() - started;
                frame.commit();
            }, "Render thread");
            renderer.start();
            renderer.join();
            Thread.sleep(250);
            recording.stop();
            recording.dump(Path.of(args[0]));
        }
    }
}

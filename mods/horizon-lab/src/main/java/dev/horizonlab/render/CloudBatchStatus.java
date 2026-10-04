package dev.horizonlab.render;

/** Render-thread counters kept independent of optional DH classes. */
public final class CloudBatchStatus {
    public static boolean requested;
    public static boolean active;
    public static long draws;
    public static long passes;

    private CloudBatchStatus() {}
}

package org.opensearch.serverless.store;

import org.opensearch.common.CheckedSupplier;

import java.io.IOException;
import java.util.Map;

/** The shell's request meter, reduced to running the work: the recorder charges no one. */
public final class StoreUsage {
    private StoreUsage() {}

    /** Who requests are charged to; nobody here. */
    public static final class Scope {
        private Scope() {}
    }

    public static <T> T split(Map<String, Long> weights, CheckedSupplier<T, IOException> work) throws IOException {
        return work.get();
    }

    public static Scope currentScope() {
        return null;
    }

    public static <T> T within(Scope scope, CheckedSupplier<T, IOException> work) throws IOException {
        return work.get();
    }
}

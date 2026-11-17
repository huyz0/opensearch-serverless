package org.opensearch.common.blobstore;

import java.util.ArrayList;
import java.util.Collections;
import java.util.Iterator;
import java.util.List;

/** Just what the node log uses of the server's BlobPath: a path of components, each added one at a time. */
public final class BlobPath implements Iterable<String> {
    private final List<String> parts;

    private BlobPath(List<String> parts) {
        this.parts = parts;
    }

    public static BlobPath cleanPath() {
        return new BlobPath(Collections.emptyList());
    }

    public BlobPath add(String part) {
        final List<String> next = new ArrayList<>(parts);
        next.add(part);
        return new BlobPath(Collections.unmodifiableList(next));
    }

    /** Each component followed by '/', as the server's does. */
    public String buildAsString() {
        final StringBuilder out = new StringBuilder();
        for (String part : parts) {
            out.append(part).append('/');
        }
        return out.toString();
    }

    @Override
    public Iterator<String> iterator() {
        return parts.iterator();
    }
}

package org.opensearch.serverless.store;

import org.opensearch.core.xcontent.XContentBuilder;
import org.opensearch.core.xcontent.XContentParser;

import java.io.IOException;

/** Recording stub: the commits here carry no digest, so the shell's own class, which needs the server's mappers, is not compiled. */
public final class PruningDigest {
    public static final PruningDigest EMPTY = new PruningDigest();

    public boolean isEmpty() {
        return true;
    }

    public void toXContent(XContentBuilder builder) throws IOException {}

    public static PruningDigest parse(XContentParser parser) throws IOException {
        parser.skipChildren();
        return EMPTY;
    }
}

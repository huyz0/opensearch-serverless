/*
 * Records what the Java shell's commit push and newest-commit messages put on
 * the wire, for crates/transport/tests/. The shell's own classes write every
 * row: each message is read from bytes by its StreamInput constructor and
 * written back by its writeTo. See README.md in this directory for how to run
 * it.
 */
package org.opensearch.serverless.transport;

import org.opensearch.common.io.stream.BytesStreamOutput;
import org.opensearch.core.common.bytes.BytesReference;
import org.opensearch.core.common.io.stream.StreamInput;
import org.opensearch.core.common.io.stream.StreamOutput;
import org.opensearch.core.common.io.stream.Writeable;
import org.opensearch.serverless.store.CommitManifest;

import java.io.IOException;
import java.io.PrintWriter;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.LinkedHashMap;
import java.util.Map;

public final class PushRecordings {

    interface Body {
        void write(StreamOutput out) throws IOException;
    }

    interface Reader<T> {
        T read(StreamInput in) throws IOException;
    }

    static String hex(BytesReference bytes) {
        final StringBuilder out = new StringBuilder();
        for (int i = 0; i < bytes.length(); i++) {
            out.append(String.format("%02x", bytes.get(i) & 0xff));
        }
        return out.length() == 0 ? "-" : out.toString();
    }

    static String text(String value) {
        if (value == null) {
            return "~";
        }
        return hex(new org.opensearch.core.common.bytes.BytesArray(value.getBytes(StandardCharsets.UTF_8)));
    }

    static BytesReference bytes(Body body) throws IOException {
        try (BytesStreamOutput out = new BytesStreamOutput()) {
            body.write(out);
            return out.bytes();
        }
    }

    static <T extends Writeable> T roundTrip(Body input, Reader<T> reader, StringBuilder written) throws IOException {
        final BytesReference in = bytes(input);
        final T value;
        try (StreamInput stream = in.streamInput()) {
            value = reader.read(stream);
            if (stream.available() != 0) {
                throw new AssertionError("bytes left after reading");
            }
        }
        final BytesReference out = bytes(value::writeTo);
        if (out.equals(in) == false) {
            throw new AssertionError("writeTo differs from what was read");
        }
        written.append(hex(out));
        return value;
    }

    static void parent(StreamOutput out, String node, long id) throws IOException {
        out.writeString(node);
        if (node.isEmpty() == false) {
            out.writeLong(id);
        }
    }

    static CommitManifest manifest(long term, String writer, long walOrdinal, long stage) {
        final Map<String, String> files = new LinkedHashMap<>();
        files.put("_0.si", "@tZ9iGQAAQACwLK7Z_____w/e=00000001794710559389/00000000000000000000#538:120");
        files.put("segments_3", "t=" + term + "/_1.seg#445:88");
        final Map<String, Long> lengths = new LinkedHashMap<>();
        lengths.put("_0.si", 120L);
        lengths.put("segments_3", 88L);
        CommitManifest m = new CommitManifest(term, files, writer, lengths, null, walOrdinal);
        return stage >= 0 ? m.withStageSequence(stage) : m;
    }

    static void pushRequests(PrintWriter out) throws IOException {
        final Object[][] cases = {
            { "", 0L, "logs", "uuid-1", 0, manifest(2, "w1", 5, 4), 0, -1L, -1L },
            { "n1", 42L, "ünï", "uuid-é", 1023, manifest(3, null, -1, -1), 2, 7L, 9L },
        };
        for (Object[] c : cases) {
            final StringBuilder written = new StringBuilder();
            final BytesReference commit = ((CommitManifest) c[5]).toBytes();
            final int blocks = (Integer) c[6];
            final CommitPushRequest r = roundTrip(o -> {
                parent(o, (String) c[0], (Long) c[1]);
                o.writeString((String) c[2]);
                o.writeString((String) c[3]);
                o.writeVInt((Integer) c[4]);
                o.writeBytesReference(commit);
                o.writeVInt(blocks);
                for (int i = 0; i < blocks; i++) {
                    o.writeString("blk-" + i);
                    o.writeByteArray(new byte[] { (byte) i, 0, (byte) 0xff, 7 });
                }
                o.writeZLong((Long) c[7]);
                o.writeZLong((Long) c[8]);
            }, CommitPushRequest::new, written);
            out.println(
                String.join(
                    "\t",
                    "push_request",
                    text(r.getParentTask().isSet() ? r.getParentTask().getNodeId() : ""),
                    Long.toString(r.getParentTask().isSet() ? r.getParentTask().getId() : 0L),
                    text(r.indexName()),
                    text(r.indexUuid()),
                    Integer.toString(r.shard()),
                    hex(commit),
                    Integer.toString(r.blocks().size()),
                    Long.toString(r.mappingVersion()),
                    Long.toString(r.settingsVersion()),
                    written.toString()
                )
            );
        }
    }

    static void pushResponses(PrintWriter out) throws IOException {
        for (boolean open : new boolean[] { false, true }) {
            final StringBuilder written = new StringBuilder();
            final CommitPushResponse r = roundTrip(new CommitPushResponse(open)::writeTo, CommitPushResponse::new, written);
            out.println(String.join("\t", "push_response", Boolean.toString(r.open()), written.toString()));
        }
    }

    static void newestRequests(PrintWriter out) throws IOException {
        final Object[][] cases = { { "", 0L, "logs", "uuid-1", 0 }, { "n1", 42L, "ünï", "uuid-é", 1023 } };
        for (Object[] c : cases) {
            final StringBuilder written = new StringBuilder();
            final NewestCommitRequest r = roundTrip(o -> {
                parent(o, (String) c[0], (Long) c[1]);
                o.writeString((String) c[2]);
                o.writeString((String) c[3]);
                o.writeVInt((Integer) c[4]);
            }, NewestCommitRequest::new, written);
            out.println(
                String.join(
                    "\t",
                    "newest_request",
                    text(r.getParentTask().isSet() ? r.getParentTask().getNodeId() : ""),
                    Long.toString(r.getParentTask().isSet() ? r.getParentTask().getId() : 0L),
                    text(r.indexName()),
                    text(r.indexUuid()),
                    Integer.toString(r.shard()),
                    written.toString()
                )
            );
        }
    }

    static void newestResponses(PrintWriter out) throws IOException {
        final CommitManifest[] cases = { null, manifest(2, "w1", 5, 4), manifest(3, null, -1, -1) };
        for (CommitManifest m : cases) {
            final StringBuilder written = new StringBuilder();
            final NewestCommitResponse r = roundTrip(new NewestCommitResponse(m)::writeTo, NewestCommitResponse::new, written);
            out.println(
                String.join(
                    "\t",
                    "newest_response",
                    r.commit() == null ? "~" : hex(r.commit().toBytes()),
                    written.toString()
                )
            );
        }
    }

    public static void main(String[] args) throws IOException {
        final Path dir = Path.of(args[0]);
        try (PrintWriter out = new PrintWriter(Files.newBufferedWriter(dir.resolve("push.txt"), StandardCharsets.UTF_8))) {
            pushRequests(out);
            pushResponses(out);
            newestRequests(out);
            newestResponses(out);
        }
    }
}

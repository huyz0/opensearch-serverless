/*
 * Records what the Java shell's forwarded-write messages and its
 * TransportAuthenticator put on the wire, for crates/transport/tests/. The
 * shell's own classes write every row: each message is read from bytes by
 * its StreamInput constructor and written back by its writeTo. See README.md
 * in this directory for how to run it.
 */
package org.opensearch.serverless.transport;

import org.opensearch.common.io.stream.BytesStreamOutput;
import org.opensearch.core.common.bytes.BytesReference;
import org.opensearch.core.common.io.stream.StreamInput;
import org.opensearch.core.common.io.stream.StreamOutput;
import org.opensearch.core.common.io.stream.Writeable;
import org.opensearch.serverless.forwarding.ForwardedBulkRequest;
import org.opensearch.serverless.forwarding.ForwardedBulkResponse;
import org.opensearch.serverless.forwarding.ForwardedIndexRequest;
import org.opensearch.serverless.forwarding.ForwardedIndexResponse;
import org.opensearch.serverless.metadata.TransportSecrets;
import org.opensearch.serverless.write.BulkOperation;
import org.opensearch.serverless.write.BulkOutcome;

import java.io.IOException;
import java.io.PrintWriter;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.TreeMap;
import java.util.concurrent.atomic.AtomicLong;

public final class ForwardingRecordings {

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

    /** A string as UTF-8 hex, `~` for null and `-` for empty. */
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

    /** Reads `input` with the Java's reader, then writes it back with its writeTo. */
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

    static void indexRequests(PrintWriter out) throws IOException {
        final Object[][] cases = {
            { "", 0L, "logs", 3, "doc-1", "{\"a\":1}", false, false, -2L, 0L, false, null, false, null },
            { "n1", 42L, "logs", 0, "doc-2", "", true, true, 7L, 3L, false, "uuid-1", true, "r1" },
            { "", 0L, "ünïcode", 1023, "é😀", "{\"b\":\"ü\"}", false, false, -2L, 0L, true, null, false, "route" },
            { "", 0L, "big", 7, "x", "y".repeat(300), true, false, Long.MAX_VALUE, Long.MAX_VALUE, false, "u", false, null },
        };
        for (Object[] c : cases) {
            final StringBuilder written = new StringBuilder();
            final ForwardedIndexRequest r = roundTrip(o -> {
                parent(o, (String) c[0], (Long) c[1]);
                o.writeString((String) c[2]);
                o.writeVInt((Integer) c[3]);
                o.writeString((String) c[4]);
                o.writeString((String) c[5]);
                o.writeBoolean((Boolean) c[6]);
                o.writeBoolean((Boolean) c[7]);
                o.writeZLong((Long) c[8]);
                o.writeVLong((Long) c[9]);
                o.writeBoolean((Boolean) c[10]);
                o.writeOptionalString((String) c[11]);
                o.writeBoolean((Boolean) c[12]);
                o.writeOptionalString((String) c[13]);
            }, ForwardedIndexRequest::new, written);
            out.println(
                String.join(
                    "\t",
                    "index_request",
                    text(r.getParentTask().isSet() ? r.getParentTask().getNodeId() : ""),
                    Long.toString(r.getParentTask().isSet() ? r.getParentTask().getId() : 0L),
                    text(r.index()),
                    Integer.toString(r.shard()),
                    text(r.id()),
                    text(r.source()),
                    Boolean.toString(r.refresh()),
                    Boolean.toString(r.deletion()),
                    Long.toString(r.ifSeqNo()),
                    Long.toString(r.ifPrimaryTerm()),
                    Boolean.toString(r.requireAbsent()),
                    text(r.indexUuid()),
                    Boolean.toString(r.isSteered()),
                    text(r.routing()),
                    written.toString()
                )
            );
        }
    }

    static void indexResponses(PrintWriter out) throws IOException {
        final ForwardedIndexResponse[] cases = {
            new ForwardedIndexResponse("node-a", 12L, 3L, 4L, true, false, false),
            new ForwardedIndexResponse("node-b", -2L, 0L, -1L, false, true, true),
            new ForwardedIndexResponse("ñode", Long.MAX_VALUE, Long.MAX_VALUE, Long.MIN_VALUE, true, true, true),
        };
        for (ForwardedIndexResponse r : cases) {
            final StringBuilder written = new StringBuilder();
            final ForwardedIndexResponse back = roundTrip(r::writeTo, ForwardedIndexResponse::new, written);
            out.println(
                String.join(
                    "\t",
                    "index_response",
                    text(back.ownerNodeId()),
                    Long.toString(back.seqNo()),
                    Long.toString(back.primaryTerm()),
                    Long.toString(back.version()),
                    Boolean.toString(back.created()),
                    Boolean.toString(back.found()),
                    Boolean.toString(back.refreshed()),
                    written.toString()
                )
            );
        }
    }

    static void bulkRequests(PrintWriter out) throws IOException {
        // id, source, deletion, ifSeqNo, ifPrimaryTerm, requireAbsent, routing
        final Object[][][] batches = {
            {},
            { { "a", "{\"x\":1}", false, -2L, 0L, false, null } },
            {
                { "a", "{\"x\":1}", false, 5L, 2L, false, "r" },
                { "b", "", true, -2L, 0L, false, null },
                { "ü😀", "{\"y\":\"é\"}", false, -2L, 0L, true, null } },
        };
        final boolean[][] flags = { { false, false, false }, { true, false, true }, { false, true, false } };
        for (int i = 0; i < batches.length; i++) {
            final Object[][] batch = batches[i];
            final boolean[] f = flags[i];
            final String uuid = i == 2 ? "uuid-2" : null;
            final StringBuilder written = new StringBuilder();
            final ForwardedBulkRequest r = roundTrip(o -> {
                parent(o, "", 0L);
                o.writeString("logs");
                o.writeVInt(2);
                o.writeVInt(batch.length);
                for (Object[] op : batch) {
                    o.writeString((String) op[0]);
                    o.writeString((String) op[1]);
                    o.writeBoolean((Boolean) op[2]);
                    o.writeZLong((Long) op[3]);
                    o.writeVLong((Long) op[4]);
                    o.writeBoolean((Boolean) op[5]);
                    o.writeOptionalString((String) op[6]);
                }
                o.writeBoolean(f[0]);
                o.writeOptionalString(uuid);
                o.writeBoolean(f[1]);
                o.writeBoolean(f[2]);
            }, ForwardedBulkRequest::new, written);
            final List<String> ops = new ArrayList<>();
            for (BulkOperation op : r.batch()) {
                ops.add(
                    String.join(
                        "|",
                        text(op.record().id()),
                        text(op.record().source()),
                        Boolean.toString(op.record().isDeletion()),
                        Long.toString(op.ifSeqNo()),
                        Long.toString(op.ifPrimaryTerm()),
                        Boolean.toString(op.requireAbsent()),
                        text(op.record().routing())
                    )
                );
            }
            out.println(
                String.join(
                    "\t",
                    "bulk_request",
                    text(r.index()),
                    Integer.toString(r.shard()),
                    ops.isEmpty() ? "-" : String.join(",", ops),
                    Boolean.toString(r.refresh()),
                    text(r.indexUuid()),
                    Boolean.toString(r.isSteered()),
                    Boolean.toString(f[2]),
                    written.toString()
                )
            );
        }
    }

    static void bulkResponses(PrintWriter out) throws IOException {
        final List<List<BulkOutcome>> cases = List.of(
            List.of(),
            List.of(
                BulkOutcome.indexed("a", 1L, 1L, 1L, true),
                BulkOutcome.indexed("b", 2L, 1L, 3L, false),
                BulkOutcome.deleted("c", true, 4L, 1L, 2L),
                BulkOutcome.deleted("d", false, 5L, 1L, -1L),
                BulkOutcome.conflicted("e", "version conflict"),
                BulkOutcome.retryable("f", "mapping update in flight"),
                BulkOutcome.failed("ü", "bad document")
            )
        );
        boolean refreshed = false;
        for (List<BulkOutcome> outcomes : cases) {
            final StringBuilder written = new StringBuilder();
            final ForwardedBulkResponse back = roundTrip(
                new ForwardedBulkResponse("owner", outcomes, refreshed)::writeTo,
                ForwardedBulkResponse::new,
                written
            );
            final List<String> rows = new ArrayList<>();
            for (BulkOutcome o : back.outcomes()) {
                rows.add(
                    String.join(
                        "|",
                        text(o.id()),
                        text(o.failure()),
                        Boolean.toString(o.conflict()),
                        Boolean.toString(o.retryable()),
                        Boolean.toString(o.isDeletion()),
                        Boolean.toString(o.found()),
                        Long.toString(o.seqNo()),
                        Long.toString(o.primaryTerm()),
                        Long.toString(o.version()),
                        Boolean.toString(o.created())
                    )
                );
            }
            out.println(
                String.join(
                    "\t",
                    "bulk_response",
                    text(back.ownerNodeId()),
                    rows.isEmpty() ? "-" : String.join(",", rows),
                    Boolean.toString(back.refreshed()),
                    written.toString()
                )
            );
            refreshed = true;
        }
    }

    static String headers(Map<String, String> headers) {
        if (headers.isEmpty()) {
            return "-";
        }
        final List<String> out = new ArrayList<>();
        for (Map.Entry<String, String> e : headers.entrySet()) {
            out.add(text(e.getKey()) + "=" + text(e.getValue()));
        }
        return String.join(",", out);
    }

    static void macs(PrintWriter out) throws IOException {
        final String k1 = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";
        final String k0 = "ffeeddccbbaa99887766554433221100ffeeddccbbaa99887766554433221100";
        final TransportSecrets secrets = new TransportSecrets(3L, k1, k0, 1_700_000_000_000L);
        final AtomicLong clock = new AtomicLong(1_700_000_005_000L);
        final TransportAuthenticator signer = new TransportAuthenticator(
            () -> secrets,
            () -> {},
            () -> "sender-node/eph-s",
            clock::get,
            () -> 30_000L
        );
        final ForwardedIndexRequest write = new ForwardedIndexRequest("logs", "uuid-1", 3, "doc-1", "{\"a\":1}", false, false, -2L, 0L, false);
        final Map<String, String> none = Map.of();
        final Map<String, String> some = new LinkedHashMap<>();
        some.put("x-opaque-id", "abc");
        some.put("_system_index_access_allowed", "true");
        // The MAC and the legacy token header are left out of the digest.
        final Map<String, String> withAuth = new TreeMap<>(some);
        withAuth.put(TransportAuthenticator.MAC_HEADER, "old");
        withAuth.put(ShardRouter.TOKEN_HEADER, "token");
        final List<Object[]> cases = List.of(
            new Object[] { ForwardedIndexRequest.ACTION, write, "recipient-node/eph-r", none },
            new Object[] { ForwardedIndexRequest.ACTION, write, "recipient-node/eph-r", some },
            new Object[] { ForwardedIndexRequest.ACTION, write, "recipient-node/eph-r", withAuth },
            new Object[] { "internal:serverless/document/bulk", write, "other/eph-2", some }
        );
        for (Object[] c : cases) {
            final String action = (String) c[0];
            final ForwardedIndexRequest request = (ForwardedIndexRequest) c[1];
            final String recipient = (String) c[2];
            @SuppressWarnings("unchecked")
            final Map<String, String> h = (Map<String, String>) c[3];
            final String header = signer.sign(action, request, recipient, h);
            // The receiver's own authenticator, as the recipient, verifies it.
            final TransportAuthenticator receiver = new TransportAuthenticator(
                () -> secrets,
                () -> {},
                () -> recipient,
                clock::get,
                () -> 30_000L
            );
            receiver.verify(action, request, header, h);
            out.println(
                String.join(
                    "\t",
                    "mac",
                    k1,
                    text(action),
                    text(recipient),
                    headers(h),
                    hex(bytes(request::writeTo)),
                    text(header)
                )
            );
        }
    }

    public static void main(String[] args) throws IOException {
        final Path dir = Path.of(args[0]);
        try (PrintWriter out = new PrintWriter(Files.newBufferedWriter(dir.resolve("forwarding.txt"), StandardCharsets.UTF_8))) {
            indexRequests(out);
            indexResponses(out);
            bulkRequests(out);
            bulkResponses(out);
        }
        try (PrintWriter out = new PrintWriter(Files.newBufferedWriter(dir.resolve("macs.txt"), StandardCharsets.UTF_8))) {
            macs(out);
        }
    }
}

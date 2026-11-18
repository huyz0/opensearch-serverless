/*
 * Records what the Java shell's tail fetch and tail push messages put on the
 * wire, for crates/transport/tests/. The shell's own classes write every row:
 * each message is written by its writeTo, read back by its StreamInput
 * constructor, and written again. See README.md in this directory for how to
 * run it.
 */
package org.opensearch.serverless.transport;

import org.opensearch.common.io.stream.BytesStreamOutput;
import org.opensearch.core.common.bytes.BytesArray;
import org.opensearch.core.common.bytes.BytesReference;
import org.opensearch.core.common.io.stream.StreamInput;
import org.opensearch.core.common.io.stream.Writeable;
import org.opensearch.core.index.Index;
import org.opensearch.core.index.shard.ShardId;
import org.opensearch.core.tasks.TaskId;
import org.opensearch.serverless.nodelog.WalRecord;

import java.io.IOException;
import java.io.PrintWriter;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.LinkedHashMap;
import java.util.LinkedHashSet;
import java.util.List;
import java.util.Map;
import java.util.Set;

public final class TailRecordings {

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
        return value == null ? "~" : hex(new BytesArray(value.getBytes(StandardCharsets.UTF_8)));
    }

    /** Writes, reads back, writes again and checks the two are the same bytes. */
    static <T extends Writeable> String roundTrip(T value, Reader<T> reader) throws IOException {
        final BytesReference first;
        try (BytesStreamOutput out = new BytesStreamOutput()) {
            value.writeTo(out);
            first = out.bytes();
        }
        final T back;
        try (StreamInput in = first.streamInput()) {
            back = reader.read(in);
            if (in.available() != 0) {
                throw new AssertionError("bytes left after reading");
            }
        }
        try (BytesStreamOutput out = new BytesStreamOutput()) {
            back.writeTo(out);
            if (out.bytes().equals(first) == false) {
                throw new AssertionError("writeTo differs from what was read");
            }
        }
        return hex(first);
    }

    static ShardId shard(String name, String uuid, int id) {
        return new ShardId(new Index(name, uuid), id);
    }

    /** A record as a column: text(id),deleted,text(source),seq,term,version,text(routing). */
    static String column(List<WalRecord> records) {
        if (records.isEmpty()) {
            return "-";
        }
        final List<String> items = new ArrayList<>();
        for (WalRecord r : records) {
            items.add(
                String.join(
                    ",",
                    text(r.id()),
                    Boolean.toString(r.isDeletion()),
                    text(r.isDeletion() ? "" : r.source()),
                    Long.toString(r.seqNo()),
                    Long.toString(r.primaryTerm()),
                    Long.toString(r.version()),
                    text(r.routing())
                )
            );
        }
        return String.join(";", items);
    }

    static List<WalRecord> records(int count) {
        final List<WalRecord> out = new ArrayList<>();
        for (int i = 0; i < count; i++) {
            final long seq = 10 + i;
            if (i % 3 == 2) {
                out.add(WalRecord.deletion("d" + i, seq, 2, 5 + i).withRouting(i % 2 == 0 ? "r" + i : null));
            } else {
                out.add(new WalRecord("ïd" + i, "{\"a\":" + i + ",\"t\":\"é\"}", seq, 2, 1 + i).withRouting(i % 2 == 1 ? "r" + i : null));
            }
        }
        return out;
    }

    static String parentNode(org.opensearch.transport.TransportRequest r) {
        return text(r.getParentTask().isSet() ? r.getParentTask().getNodeId() : "");
    }

    static String parentId(org.opensearch.transport.TransportRequest r) {
        return Long.toString(r.getParentTask().isSet() ? r.getParentTask().getId() : 0L);
    }

    static void fetchRequests(PrintWriter out) throws IOException {
        final Object[][] cases = {
            { "", 0L, "logs", "uuid-1", 0, -1L, "n1", TailFetchRequest.NEW },
            { "n1", 42L, "ünï", "uuid-é", 1023, 17L, "ünï-node", 3L },
            { "", 0L, "logs", "uuid-1", 7, 1L << 40, "n2", TailFetchRequest.CHECK },
        };
        for (Object[] c : cases) {
            final TailFetchRequest r = new TailFetchRequest(
                shard((String) c[2], (String) c[3], (Integer) c[4]),
                (Long) c[5],
                (String) c[6],
                (Long) c[7]
            );
            if (((String) c[0]).isEmpty() == false) {
                r.setParentTask(new TaskId((String) c[0], (Long) c[1]));
            }
            out.println(
                String.join(
                    "\t",
                    "fetch_request",
                    parentNode(r),
                    parentId(r),
                    text(r.shardId().getIndexName()),
                    text(r.shardId().getIndex().getUUID()),
                    Integer.toString(r.shardId().id()),
                    Long.toString(r.afterSeqNo()),
                    text(r.subscriber()),
                    Long.toString(r.subscription()),
                    roundTrip(r, TailFetchRequest::new)
                )
            );
        }
    }

    static void fetchResponses(PrintWriter out) throws IOException {
        final Object[][] cases = {
            { 0, TailFetchResponse.NONE, false, -1L },
            { 5, 4L, true, 20L },
            { 0, TailFetchResponse.NONE, false, Long.MAX_VALUE },
            { 1, 0L, true, 0L },
        };
        for (Object[] c : cases) {
            final TailFetchResponse r = new TailFetchResponse(records((Integer) c[0]), (Long) c[1], (Boolean) c[2], (Long) c[3]);
            out.println(
                String.join(
                    "\t",
                    "fetch_response",
                    column(r.records()),
                    Long.toString(r.subscription()),
                    Boolean.toString(r.complete()),
                    Long.toString(r.claimedThrough()),
                    roundTrip(r, TailFetchResponse::new)
                )
            );
        }
    }

    static void pushRequests(PrintWriter out) throws IOException {
        final Map<ShardId, List<WalRecord>> one = new LinkedHashMap<>();
        one.put(shard("logs", "uuid-1", 0), records(3));
        final Map<ShardId, List<WalRecord>> several = new LinkedHashMap<>();
        several.put(shard("logs", "uuid-1", 0), records(2));
        several.put(shard("ünï", "uuid-é", 1023), records(0));
        several.put(shard("metrics", "uuid-2", 4), records(4));
        final List<Map<ShardId, List<WalRecord>>> cases = List.of(one, several, new LinkedHashMap<>());
        for (Map<ShardId, List<WalRecord>> c : cases) {
            final TailPushRequest r = new TailPushRequest(c);
            final List<String> shards = new ArrayList<>();
            for (Map.Entry<ShardId, List<WalRecord>> e : r.records().entrySet()) {
                shards.add(
                    String.join(
                        "|",
                        text(e.getKey().getIndexName()),
                        text(e.getKey().getIndex().getUUID()),
                        Integer.toString(e.getKey().id()),
                        column(e.getValue())
                    )
                );
            }
            out.println(
                String.join(
                    "\t",
                    "push_request",
                    parentNode(r),
                    parentId(r),
                    shards.isEmpty() ? "-" : String.join("/", shards),
                    roundTrip(r, TailPushRequest::new)
                )
            );
        }
    }

    static void pushResponses(PrintWriter out) throws IOException {
        final Set<ShardId> some = new LinkedHashSet<>();
        some.add(shard("logs", "uuid-1", 0));
        some.add(shard("ünï", "uuid-é", 1023));
        for (Set<ShardId> c : List.of(Set.<ShardId>of(), some)) {
            final TailPushResponse r = new TailPushResponse(c);
            final List<String> shards = new ArrayList<>();
            for (ShardId s : r.unwanted()) {
                shards.add(String.join("|", text(s.getIndexName()), text(s.getIndex().getUUID()), Integer.toString(s.id())));
            }
            out.println(
                String.join("\t", "push_response", shards.isEmpty() ? "-" : String.join("/", shards), roundTrip(r, TailPushResponse::new))
            );
        }
    }

    public static void main(String[] args) throws IOException {
        final Path dir = Path.of(args[0]);
        try (PrintWriter out = new PrintWriter(Files.newBufferedWriter(dir.resolve("tail.txt"), StandardCharsets.UTF_8))) {
            fetchRequests(out);
            fetchResponses(out);
            pushRequests(out);
            pushResponses(out);
        }
    }
}

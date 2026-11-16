package org.opensearch.serverless.nodelog;

import java.io.ByteArrayInputStream;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.HashMap;
import java.util.HexFormat;
import java.util.List;
import java.util.Map;

/**
 * Reads each line of records.txt and records-strict.txt with the shell's own
 * WalRecord.fromStream and writes toBytes of what it read, or "refused",
 * checking each written record reads back to the same bytes. Then builds each
 * case of blob-cases.txt with NodeLogBlob.encode or encodeFence, writes its
 * bytes in hex (or "refused"), and what NodeLogBlob.decode reads back from
 * them, checking every slice parses back to the records it was given.
 */
public class NodeLogs {
    static WalRecord parse(String json) throws Exception {
        return WalRecord.fromStream(new ByteArrayInputStream(json.getBytes(StandardCharsets.UTF_8)));
    }

    static List<String> records(Path dir, String in, Map<Integer, WalRecord> read) throws Exception {
        final List<String> lines = new ArrayList<>();
        int number = 0;
        for (String line : Files.readAllLines(dir.resolve(in), StandardCharsets.UTF_8)) {
            number++;
            String out;
            try {
                final WalRecord record = parse(line);
                out = record.toBytes().utf8ToString();
                if (parse(out).toBytes().utf8ToString().equals(out) == false) {
                    throw new AssertionError("does not read back: " + out);
                }
                if (read != null) {
                    read.put(number, record);
                }
            } catch (AssertionError e) {
                throw e;
            } catch (Exception e) {
                out = "refused";
            }
            lines.add(out);
        }
        return lines;
    }

    static NodeLogBlob.ShardKey key(String uuid, String name, String shard) {
        return new NodeLogBlob.ShardKey(uuid, Integer.parseInt(shard), name.equals("-") ? null : name);
    }

    static String nameOf(NodeLogBlob.ShardKey key) {
        return key.indexName() == null ? "-" : key.indexName();
    }

    static String dump(NodeLogBlob.Entry entry) {
        if (entry instanceof NodeLogBlob.Fence fence) {
            return "fence by=" + fence.fencedBy() + " term=" + fence.term();
        }
        final NodeLogBlob.Header h = (NodeLogBlob.Header) entry;
        final StringBuilder out = new StringBuilder("records node=").append(h.nodeId())
            .append(" epoch=").append(h.epoch())
            .append(" ordinal=").append(h.ordinal())
            .append(" trim=").append(h.trimPoint())
            .append(" body=").append(h.bodyOffset())
            .append(" slices=[");
        String sep = "";
        for (NodeLogBlob.Slice s : h.slices()) {
            out.append(sep).append(s.shard().indexUuid()).append('/').append(nameOf(s.shard())).append('/').append(s.shard().shard())
                .append('/').append(s.term()).append('/').append(s.offset()).append('/').append(s.length())
                .append('/').append(s.records()).append('/').append(s.maxSeqNo());
            sep = ",";
        }
        out.append("] index=[");
        sep = "";
        for (Map.Entry<NodeLogBlob.ShardKey, List<NodeLogBlob.Range>> e : h.index().entrySet()) {
            out.append(sep).append(e.getKey().indexUuid()).append('/').append(nameOf(e.getKey())).append('/').append(e.getKey().shard())
                .append('=');
            String rsep = "";
            for (NodeLogBlob.Range r : e.getValue()) {
                out.append(rsep).append(r.from()).append('-').append(r.to());
                rsep = ";";
            }
            sep = ",";
        }
        return out.append(']').toString();
    }

    public static void main(String[] args) throws Exception {
        final Path dir = Path.of(args[0]);
        final Map<Integer, WalRecord> read = new HashMap<>();
        Files.write(dir.resolve("records-parsed.txt"), records(dir, "records.txt", read), StandardCharsets.UTF_8);
        Files.write(dir.resolve("records-strict-parsed.txt"), records(dir, "records-strict.txt", null), StandardCharsets.UTF_8);

        final List<String> blobs = new ArrayList<>();
        final List<String> decoded = new ArrayList<>();
        String[] blob = null;
        final Map<NodeLogBlob.ShardKey, List<NodeLogBlob.Range>> prior = new HashMap<>();
        final List<NodeLogBlob.ShardBatch> batches = new ArrayList<>();
        for (String line : Files.readAllLines(dir.resolve("blob-cases.txt"), StandardCharsets.UTF_8)) {
            final String[] t = line.split(" ");
            if (line.isEmpty() || line.startsWith("#")) {
                continue;
            }
            switch (t[0]) {
                case "blob" -> blob = t;
                case "prior" -> {
                    final List<NodeLogBlob.Range> runs = new ArrayList<>();
                    for (String run : t[4].split(",")) {
                        final String[] ends = run.split("-");
                        runs.add(new NodeLogBlob.Range(Long.parseLong(ends[0]), Long.parseLong(ends[1])));
                    }
                    prior.put(key(t[1], t[2], t[3]), runs);
                }
                case "batch" -> {
                    final List<WalRecord> records = new ArrayList<>();
                    if (t[5].equals("none") == false) {
                        for (String n : t[5].split(",")) {
                            records.add(read.get(Integer.parseInt(n)));
                        }
                    }
                    batches.add(new NodeLogBlob.ShardBatch(key(t[1], t[2], t[3]), Long.parseLong(t[4]), records));
                }
                case "end" -> {
                    NodeLogBlob.Encoded encoded = null;
                    try {
                        encoded = NodeLogBlob.encode(
                            blob[1], Long.parseLong(blob[2]), Long.parseLong(blob[3]), Long.parseLong(blob[4]), batches, prior);
                    } catch (Exception e) {
                        blobs.add("refused");
                        decoded.add("refused");
                    }
                    if (encoded != null) {
                        blobs.add(HexFormat.of().formatHex(encoded.bytes()));
                        if (NodeLogBlob.headerEnd(encoded.bytes()) != encoded.header().bodyOffset()) {
                            throw new AssertionError("header end");
                        }
                        final NodeLogBlob.Header h = (NodeLogBlob.Header) NodeLogBlob.decode(encoded.bytes());
                        decoded.add(dump(h));
                        int i = 0;
                        for (NodeLogBlob.ShardBatch batch : batches) {
                            if (batch.records().isEmpty()) {
                                continue;
                            }
                            final NodeLogBlob.Slice s = h.slices().get(i++);
                            final byte[] bytes = java.util.Arrays.copyOfRange(
                                encoded.bytes(), (int) (h.bodyOffset() + s.offset()), (int) (h.bodyOffset() + s.offset() + s.length()));
                            final List<WalRecord> back = NodeLogBlob.parseSlice(bytes);
                            for (int r = 0; r < back.size(); r++) {
                                if (back.get(r).toBytes().utf8ToString().equals(batch.records().get(r).toBytes().utf8ToString()) == false) {
                                    throw new AssertionError("slice does not read back");
                                }
                            }
                            if (back.size() != batch.records().size()) {
                                throw new AssertionError("slice record count");
                            }
                        }
                    }
                    prior.clear();
                    batches.clear();
                }
                case "fence" -> {
                    final byte[] fence = NodeLogBlob.encodeFence(t[1], Long.parseLong(t[2]));
                    blobs.add(HexFormat.of().formatHex(fence));
                    decoded.add(dump(NodeLogBlob.decode(fence)));
                }
                default -> throw new AssertionError("unknown line: " + line);
            }
        }
        Files.write(dir.resolve("blobs.txt"), blobs, StandardCharsets.UTF_8);
        Files.write(dir.resolve("decoded.txt"), decoded, StandardCharsets.UTF_8);
    }
}

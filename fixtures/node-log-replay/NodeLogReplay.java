package org.opensearch.serverless.nodelog;

import org.opensearch.common.blobstore.BlobContainer;
import org.opensearch.common.blobstore.BlobPath;
import org.opensearch.common.blobstore.BlobStore;
import org.opensearch.serverless.nodelog.NodeLogBlob.ShardKey;

import java.io.ByteArrayInputStream;
import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.NoSuchFileException;
import java.nio.file.Path;
import java.nio.file.StandardOpenOption;
import java.util.ArrayList;
import java.util.HexFormat;
import java.util.List;
import java.util.TreeMap;
import java.util.stream.Stream;

/**
 * Writes a node's log on a filesystem store with the shell's own NodeLogCommitter, as a node that stops without
 * publishing leaves it, and copies the tree to log/. Then fences and replays it with the shell's own NodeLogTakeover,
 * as a successor would, and writes what it read to replay.txt and the fences it wrote to fences.txt.
 */
public class NodeLogReplay {
    /** The filesystem store's layout: each path component a directory, each blob a file named for it. */
    record FsStore(Path root) implements BlobStore {
        @Override
        public BlobContainer blobContainer(BlobPath path) {
            Path dir = root;
            for (String part : path) {
                dir = dir.resolve(part);
            }
            return new FsContainer(dir);
        }
    }

    record FsContainer(Path dir) implements BlobContainer {
        @Override
        public boolean blobExists(String name) {
            return Files.exists(dir.resolve(name));
        }

        @Override
        public InputStream readBlob(String name) throws IOException {
            return Files.newInputStream(dir.resolve(name));
        }

        @Override
        public InputStream readBlob(String name, long position, long length) throws IOException {
            final byte[] all = Files.readAllBytes(dir.resolve(name));
            final int from = (int) Math.min(position, all.length);
            final int to = (int) Math.min(all.length, from + length);
            return new ByteArrayInputStream(all, from, to - from);
        }

        @Override
        public void writeBlob(String name, InputStream in, long size, boolean failIfAlreadyExists) throws IOException {
            Files.createDirectories(dir);
            // CREATE_NEW throws FileAlreadyExistsException for a taken slot, as the shell's FsBlobContainer does.
            try (OutputStream out = Files.newOutputStream(dir.resolve(name), StandardOpenOption.CREATE_NEW)) {
                in.transferTo(out);
            }
        }

        @Override
        public void deleteBlobsIgnoringIfNotExists(List<String> names) throws IOException {
            for (String name : names) {
                Files.deleteIfExists(dir.resolve(name));
            }
        }
    }

    static final ShardKey A = new ShardKey("logs-uuid-000000000000", 0, "logs");
    static final ShardKey B = new ShardKey("logs-uuid-000000000000", 1, "logs");
    static final ShardKey C = new ShardKey("metrics-uuid-0000000000", 0, "metrics");
    static final ShardKey NONE = new ShardKey("metrics-uuid-0000000000", 7, "metrics");

    static WalRecord doc(String id, long seq, String source) {
        return new WalRecord(id, source, seq, 1, seq + 1);
    }

    static List<WalRecord> one(WalRecord record) {
        return List.of(record);
    }

    static TreeMap<String, byte[]> tree(Path root) throws IOException {
        final TreeMap<String, byte[]> out = new TreeMap<>();
        try (Stream<Path> files = Files.walk(root)) {
            for (Path file : (Iterable<Path>) files.filter(Files::isRegularFile)::iterator) {
                out.put(root.relativize(file).toString().replace('\\', '/'), Files.readAllBytes(file));
            }
        }
        return out;
    }

    public static void main(String[] args) throws Exception {
        final Path out = Path.of(args[0]);
        final Path work = Files.createTempDirectory("node-log-replay");
        final NodeLogStore store = new NodeLogStore(new FsStore(work), BlobPath.cleanPath(), "node-1");
        final List<List<NodeLogEpoch>> leases = new ArrayList<>();
        final NodeLogCommitter committer = new NodeLogCommitter(store, 7, leases::add, (shard, term, epoch) -> {}, 0, 1 << 20, 1000);

        // Epoch 7: a blob each, then one blob three shards share, then a foreign fence closes the epoch.
        committer.commit(A, 1, one(doc("a0", 0, "{\"n\":0}")));
        final NodeLogCommitter.Position b0 = committer.commit(B, 1, one(doc("b0", 0, "{\"n\":\"b\"}")));
        final List<NodeLogCommitter.Position> a1 = new ArrayList<>();
        committer.enqueue(A, 1, one(doc("a1", 1, "{\"n\":1}")), a1::add);
        committer.enqueue(C, 1, List.of(doc("c0", 0, "{\"m\":[1,2]}"), WalRecord.deletion("c0", 1, 1, 2)), at -> {});
        committer.enqueue(B, 1, one(doc("b1", 1, "{\"n\":\"b1\"}")), at -> {});
        committer.flushQueued();
        store.fence(7, 0, "someone", 1);
        try {
            committer.commit(A, 1, one(doc("refused", 2, "{}")));
            throw new AssertionError("a fenced epoch took a write");
        } catch (IOException expected) {
            // The epoch is closed to this writer; the next write opens another.
        }
        // Epoch 8: past U+FFFF, escapes, and a larger source.
        committer.commit(A, 1, one(doc("a2", 2, "{\"t\":\"caf\u00e9 \ud83d\ude00 \\\"q\\\" \\n\"}")));
        committer.commit(B, 1, one(doc("b2", 2, "{\"pad\":\"" + "x".repeat(5000) + "\"}")));
        committer.commit(C, 2, one(doc("c2", 2, "{\"m\":3}")));
        // A published through its shared blob and B its first: what a successor still replays is what is left.
        committer.onPublished(A, a1.get(0));
        committer.onPublished(B, b0);
        final List<NodeLogEpoch> epochs = committer.positions();

        final TreeMap<String, byte[]> written = tree(work);
        final Path log = out.resolve("log");
        for (var blob : written.entrySet()) {
            final Path file = log.resolve(blob.getKey());
            Files.createDirectories(file.getParent());
            Files.write(file, blob.getValue());
        }
        final List<String> lease = new ArrayList<>();
        lease.add("# epoch trim landed: the predecessor's lease as positions() reports it");
        for (NodeLogEpoch epoch : epochs) {
            lease.add(epoch.epoch() + " " + epoch.trimPoint() + " " + epoch.landed());
        }
        Files.write(out.resolve("lease.txt"), lease, StandardCharsets.UTF_8);

        final NodeLogTakeover takeover = new NodeLogTakeover(store, epochs, "node-2", 2);
        final List<String> replay = new ArrayList<>();
        replay.add("# shard: index-uuid shard index-name, then max-seq-no, holds, and each record's toBytes");
        takeover.fenceAll();
        for (var end : new TreeMap<>(takeover.ends()).entrySet()) {
            replay.add("end " + end.getKey() + " " + end.getValue());
        }
        for (ShardKey shard : List.of(A, B, C, NONE)) {
            replay.add("shard " + shard.indexUuid() + " " + shard.shard() + " " + shard.indexName());
            replay.add("max " + takeover.maxSeqNoOf(shard));
            replay.add("holds " + takeover.holds(shard));
            takeover.forEachRecord(shard, record -> replay.add("record " + record.toBytes().utf8ToString()));
        }
        Files.write(out.resolve("replay.txt"), replay, StandardCharsets.UTF_8);

        final List<String> fences = new ArrayList<>();
        fences.add("# key hex: each blob the takeover wrote");
        for (var blob : tree(work).entrySet()) {
            if (written.containsKey(blob.getKey()) == false) {
                fences.add(blob.getKey() + " " + HexFormat.of().formatHex(blob.getValue()));
            }
        }
        Files.write(out.resolve("fences.txt"), fences, StandardCharsets.UTF_8);
    }
}

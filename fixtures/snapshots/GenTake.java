import org.opensearch.common.io.stream.BytesStreamOutput;
import org.opensearch.core.common.bytes.BytesReference;
import org.opensearch.serverless.repository.SnapshotRecord;
import org.opensearch.serverless.store.CommitManifest;
import java.nio.file.*;
import java.util.*;

/** Writes a finished shallow snapshot the way a Java node takes one: its register value, for a Rust restore to read. */
public class GenTake {
    public static void main(String[] a) throws Exception {
        Path dir = Path.of(a[0]);
        Files.createDirectories(dir);
        Map<String, String> files = new LinkedHashMap<>();
        files.put("_0.si", "t=3");
        files.put("_0.cfs", "t=3/pack#4:6");
        CommitManifest commit = new CommitManifest(3L, files, "writer-1", Map.of("_0.si", 10L, "_0.cfs", 6L));
        Map<Integer, CommitManifest> shards = new LinkedHashMap<>();
        shards.put(0, commit);
        Map<String, SnapshotRecord.SnapshottedIndex> indices = new LinkedHashMap<>();
        indices.put("logs", new SnapshotRecord.SnapshottedIndex("uuidjavalogs000000000A", 1, "{\"properties\":{\"n\":{\"type\":\"long\"}}}", shards));
        SnapshotRecord done = new SnapshotRecord("backups", "java-1", "snapuuid00000000000003", true, 1700000001000L, 1700000002000L, indices)
            .withStorageGeneration("gen0000000000000000003");
        try (var out = new BytesStreamOutput()) {
            out.writeString("v2-marker#hash#" + done.storageGeneration());
            out.writeBytesReference(done.toBytes());
            out.writeBoolean(true);
            Files.write(dir.resolve("state.bin"), BytesReference.toBytes(out.bytes()));
        }
    }
}

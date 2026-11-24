import org.opensearch.common.io.stream.BytesStreamOutput;
import org.opensearch.core.common.bytes.BytesReference;
import org.opensearch.serverless.repository.RepositoryDescriptor;
import org.opensearch.serverless.repository.SnapshotRecord;
import org.opensearch.serverless.store.CommitManifest;
import java.nio.file.*;
import java.util.*;

public class Gen {
    static void write(Path dir, String name, BytesReference b) throws Exception {
        Files.write(dir.resolve(name), BytesReference.toBytes(b));
    }
    public static void main(String[] a) throws Exception {
        Path dir = Path.of(a[0]);
        Files.createDirectories(dir);
        Map<String, Object> settings = new LinkedHashMap<>();
        settings.put("location", "backups");
        settings.put("remote_store_index_shallow_copy", true);
        settings.put("max_snapshot_bytes_per_sec", 5);
        write(dir, "repository.json", new RepositoryDescriptor("backups", "fs", settings, 1700000000000L).toBytes());

        Map<String, String> files = new LinkedHashMap<>();
        files.put("_0.cfs", "t=2/_0-abc.seg#10:100");
        files.put("segments_1", "t=2/_0-abc.seg#110:50");
        CommitManifest commit = new CommitManifest(2L, files, "writer-1", Map.of("_0.cfs", 100L, "segments_1", 50L));
        Map<Integer, CommitManifest> shards = new LinkedHashMap<>();
        shards.put(0, commit);
        Map<String, SnapshotRecord.SnapshottedIndex> indices = new LinkedHashMap<>();
        indices.put("logs", new SnapshotRecord.SnapshottedIndex("uuidlogs0000000000000A", 1, "{\"properties\":{\"n\":{\"type\":\"long\"}}}", shards));
        indices.put("metrics", new SnapshotRecord.SnapshottedIndex("uuidmetrics00000000B", 3, null, new LinkedHashMap<>()));
        SnapshotRecord done = new SnapshotRecord("backups", "daily-1", "snapuuid00000000000001", false, 1700000001000L, 1700000002000L,
            Map.of("logs", indices.get("logs"))).withStorageGeneration("gen0000000000000000001");
        write(dir, "snapshot-finished.json", done.toBytes());
        SnapshotRecord open = new SnapshotRecord("backups", "daily-2", "snapuuid00000000000002", true, 1700000003000L, 0L, indices)
            .withStorageGeneration("gen0000000000000000002");
        write(dir, "snapshot-in-progress.json", open.toBytes());
        // The state register's value as SnapshotStore.encode writes it.
        for (var e : Map.of("state-finished.bin", done, "state-in-progress.bin", open).entrySet()) {
            try (var out = new BytesStreamOutput()) {
                out.writeString("v2-marker#hash#" + e.getValue().storageGeneration());
                out.writeBytesReference(e.getValue().toBytes());
                out.writeBoolean(e.getKey().contains("finished"));
                write(dir, e.getKey(), out.bytes());
            }
        }
    }
}

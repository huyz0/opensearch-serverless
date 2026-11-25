import java.nio.file.Files;
import java.nio.file.Path;
import java.util.List;
import java.util.Map;
import org.opensearch.core.common.bytes.BytesReference;
import org.opensearch.core.index.Index;
import org.opensearch.core.index.shard.ShardId;
import org.opensearch.snapshots.SnapshotId;
import org.opensearch.snapshots.SnapshotInfo;
import org.opensearch.snapshots.SnapshotShardFailure;
import org.opensearch.repositories.blobstore.ChecksumBlobStoreFormat;

/**
 * Writes a snapshot's own blob (snap-&lt;uuid&gt;.dat) with the server's own
 * BlobStoreRepository.SNAPSHOT_FORMAT: a clean snapshot, one that includes global
 * state, one with a failed shard and one that is a shallow remote-store copy.
 *
 * Usage: java GenSnapshotInfo <out-dir>
 */
public class GenSnapshotInfo {
    public static void main(String[] a) throws Exception {
        Path dir = Path.of(a[0]);
        SnapshotId id = new SnapshotId("snap-1", "SnapUuidAaaaBbbbCccc01");
        List<String> indices = List.of("logs", "metrics");
        write(dir, "snapshot-info.ok", new SnapshotInfo(id, indices, List.of(), 1_700_000_000_000L, null, 1_700_000_005_000L, 3, List.of(), false, Map.of("who", "ci"), false), id);
        write(dir, "snapshot-info.global", new SnapshotInfo(id, indices, List.of(), 1_700_000_000_000L, null, 1_700_000_005_000L, 3, List.of(), true, null, false), id);
        SnapshotShardFailure failure = new SnapshotShardFailure("node-1", new ShardId(new Index("logs", "idLogsAaaa"), 1), "disk full");
        write(dir, "snapshot-info.partial", new SnapshotInfo(id, indices, List.of(), 1_700_000_000_000L, null, 1_700_000_005_000L, 3, List.of(failure), false, null, false), id);
        write(dir, "snapshot-info.shallow", new SnapshotInfo(id, indices, List.of(), 1_700_000_000_000L, null, 1_700_000_005_000L, 3, List.of(), false, null, true), id);
    }

    static void write(Path dir, String stem, SnapshotInfo info, SnapshotId id) throws Exception {
        BytesReference plain = org.opensearch.repositories.blobstore.BlobStoreRepository.SNAPSHOT_FORMAT.serialize(info, "snap-" + id.getUUID(), new org.opensearch.core.compress.NoneCompressor(), ChecksumBlobStoreFormat.SNAPSHOT_ONLY_FORMAT_PARAMS);
        Files.write(dir.resolve(stem + ".dat"), BytesReference.toBytes(plain));
    }
}

import org.opensearch.core.common.bytes.BytesArray;
import org.opensearch.serverless.repository.SnapshotRecord;
import org.opensearch.serverless.store.CommitManifest;
import org.opensearch.serverless.store.FileLocation;
import java.io.ByteArrayInputStream;
import java.nio.file.*;
import java.util.*;

/** Reads what a Rust node wrote for a take, as a Java node would: the register value and the protective marker. */
public class Check {
    static void require(boolean ok, String what) {
        if (!ok) {
            throw new AssertionError(what);
        }
        System.out.println("ok: " + what);
    }

    public static void main(String[] a) throws Exception {
        Path dir = Path.of(a[0]);
        try (var input = new BytesArray(Files.readAllBytes(dir.resolve("state.bin"))).streamInput()) {
            final String marker = input.readString();
            final SnapshotRecord record = SnapshotRecord.fromStream(input.readBytesReference().streamInput());
            final boolean finished = input.readBoolean();
            require(input.available() == 0, "the register value has no bytes left over");
            require(finished, "the register holds a finished snapshot");
            require(marker.startsWith("v2-") && marker.endsWith("#" + record.storageGeneration()), "the marker names this lifetime: " + marker);
            require(record.key().equals("backups#daily-1"), "the key is repo#name");
            require(record.shallow(), "the snapshot is shallow");
            require(SnapshotRecord.inProgress(record) == false, "a finished record is not in progress");
            final var index = record.indices().get("logs");
            require(index != null && index.numberOfShards() == 2 && index.shards().size() == 2, "logs has both shards captured");
            for (Map.Entry<Integer, CommitManifest> shard : index.shards().entrySet()) {
                final CommitManifest commit = shard.getValue();
                require(commit.term() == 4, "shard " + shard.getKey() + " term");
                require(commit.files().size() == 3 && commit.lengths().size() == 3, "shard " + shard.getKey() + " files and lengths");
                for (String file : commit.files().keySet()) {
                    final FileLocation location = commit.location(file);
                    require(location != null && location.referencedBlob() != null, file + " resolves to " + location.referencedBlob());
                    require(commit.lengthOf(file) != null, file + " has a length");
                }
            }
            // The Java writes the record it read back to the same content.
            final SnapshotRecord again = SnapshotRecord.fromStream(new ByteArrayInputStream(org.opensearch.core.common.bytes.BytesReference.toBytes(record.toBytes())));
            require(again.indices().get("logs").shards().get(1).files().equals(index.shards().get(1).files()), "the record survives a Java round trip");
        }
        final SnapshotRecord protective = SnapshotRecord.fromStream(new ByteArrayInputStream(Files.readAllBytes(dir.resolve("marker.json"))));
        require(SnapshotRecord.inProgress(protective), "the protective marker reads as in progress");
        require(protective.indices().get("logs").shards().isEmpty(), "the marker names no commits");
        System.out.println("PASS");
    }
}

import java.nio.file.Files;
import java.nio.file.Path;
import java.util.Map;
import org.opensearch.Version;
import org.opensearch.common.xcontent.XContentFactory;
import org.opensearch.core.xcontent.XContentBuilder;
import org.opensearch.repositories.IndexId;
import org.opensearch.repositories.RepositoryData;
import org.opensearch.repositories.ShardGenerations;
import org.opensearch.snapshots.SnapshotId;
import org.opensearch.snapshots.SnapshotState;

/**
 * Writes `index-N` with OpenSearch's own RepositoryData.snapshotsToXContent:
 * two snapshots over two indices, one snapshot holding both, the other one,
 * with metadata generations and identifiers.
 *
 * Usage: java GenRepositoryData <out-dir>
 */
public class GenRepositoryData {
    public static void main(String[] a) throws Exception {
        IndexId logs = new IndexId("logs", "Zq1_aBcD-EfGhIjKlMnOpQ");
        IndexId users = new IndexId("users", "u2ZyXwVuTsRqPoNmLkJiHg");
        SnapshotId s1 = new SnapshotId("snap-1", "Aa11Bb22Cc33Dd44Ee55Ff");
        SnapshotId s2 = new SnapshotId("snap-2", "Gg66Hh77Ii88Jj99Kk00Ll");

        ShardGenerations g1 = ShardGenerations.builder()
            .put(logs, 0, "genLogs0").put(logs, 1, "genLogs1").put(users, 0, "genUsers0").build();
        ShardGenerations g2 = ShardGenerations.builder()
            .put(logs, 0, "genLogs0b").put(logs, 1, "genLogs1b").build();

        RepositoryData data = RepositoryData.EMPTY
            .addSnapshot(s1, SnapshotState.SUCCESS, Version.CURRENT, g1,
                Map.of(logs, "mdLogs1", users, "mdUsers1"),
                Map.of("mdLogs1", "idLogsAaaa", "mdUsers1", "idUsersBbbb"))
            .addSnapshot(s2, SnapshotState.SUCCESS, Version.CURRENT, g2,
                Map.of(logs, "mdLogs2"),
                Map.of("mdLogs2", "idLogsCccc"));
        try (XContentBuilder b = XContentFactory.jsonBuilder()) {
            data.snapshotsToXContent(b, Version.CURRENT);
            Files.writeString(Path.of(a[0]).resolve("index-7.json"), b.toString());
        }
    }
}

import java.nio.file.Files;
import java.nio.file.Path;
import java.util.List;
import org.apache.lucene.util.Version;
import org.opensearch.cluster.metadata.AliasMetadata;
import org.opensearch.cluster.metadata.IndexMetadata;
import org.opensearch.common.settings.Settings;
import org.opensearch.common.unit.TimeValue;
import org.opensearch.core.common.bytes.BytesReference;
import org.opensearch.core.common.unit.ByteSizeValue;
import org.opensearch.core.compress.Compressor;
import org.opensearch.common.compress.DeflateCompressor;
import org.opensearch.index.snapshots.blobstore.BlobStoreIndexShardSnapshot;
import org.opensearch.index.store.StoreFileMetadata;
import org.opensearch.repositories.blobstore.BlobStoreRepository;
import org.opensearch.repositories.blobstore.ChecksumBlobStoreFormat;

/**
 * Writes an index's metadata blob and a shard's snapshot blob with the
 * server's own formats (BlobStoreRepository.INDEX_METADATA_FORMAT and
 * INDEX_SHARD_SNAPSHOT_FORMAT), each plain and deflate-compressed.
 *
 * Usage: java GenMetadata <out-dir>
 */
public class GenMetadata {
    public static void main(String[] a) throws Exception {
        Path dir = Path.of(a[0]);

        IndexMetadata meta = IndexMetadata.builder("logs")
            .settings(Settings.builder()
                .put("index.version.created", org.opensearch.Version.CURRENT.id)
                .put("index.number_of_shards", 2)
                .put("index.number_of_replicas", 1)
                .put("index.uuid", "Zq1_aBcD-EfGhIjKlMnOpQ")
                .put("index.refresh_interval", "5s")
                .build())
            .putMapping("{\"_doc\":{\"properties\":{\"msg\":{\"type\":\"text\"},\"level\":{\"type\":\"keyword\"},\"n\":{\"type\":\"long\"}}}}")
            .putAlias(AliasMetadata.builder("logs-alias").build())
            .build();
        write(dir, "index-metadata", BlobStoreRepository.INDEX_METADATA_FORMAT, meta, "meta-idLogsAaaa");

        StoreFileMetadata big = new StoreFileMetadata("_0.cfs", 12_000_000L, "1xsyk1b", Version.LATEST, new org.apache.lucene.util.BytesRef(new byte[] {1, 2, 3}));
        StoreFileMetadata small = new StoreFileMetadata("segments_3", 300L, "abcdef", Version.LATEST);
        BlobStoreIndexShardSnapshot shard = new BlobStoreIndexShardSnapshot(
            "snap-1", 4L,
            List.of(
                new BlobStoreIndexShardSnapshot.FileInfo("__bigBlobUuid1", big, new ByteSizeValue(5_000_000L)),
                new BlobStoreIndexShardSnapshot.FileInfo("__smallBlobUuid2", small, null)),
            1_700_000_000_000L, 1234L, 2, 12_000_300L);
        write(dir, "snapshot", BlobStoreRepository.INDEX_SHARD_SNAPSHOT_FORMAT, shard, "snap-Aa11Bb22Cc33Dd44Ee55Ff");
    }

    static <T extends org.opensearch.core.xcontent.ToXContent> void write(Path dir, String stem, ChecksumBlobStoreFormat<T> format, T obj, String name) throws Exception {
        BytesReference plain = format.serialize(obj, name, new org.opensearch.core.compress.NoneCompressor(), ChecksumBlobStoreFormat.SNAPSHOT_ONLY_FORMAT_PARAMS);
        Files.write(dir.resolve(stem + ".plain.dat"), BytesReference.toBytes(plain));
        BytesReference packed = format.serialize(obj, name, new DeflateCompressor(), ChecksumBlobStoreFormat.SNAPSHOT_ONLY_FORMAT_PARAMS);
        Files.write(dir.resolve(stem + ".deflate.dat"), BytesReference.toBytes(packed));
    }
}

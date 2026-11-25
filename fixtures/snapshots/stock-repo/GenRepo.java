import java.nio.ByteBuffer;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.List;
import java.util.Map;
import java.util.zip.CRC32;
import org.apache.lucene.util.Version;
import org.opensearch.cluster.metadata.IndexMetadata;
import org.opensearch.common.settings.Settings;
import org.opensearch.core.common.bytes.BytesReference;
import org.opensearch.core.common.unit.ByteSizeValue;
import org.opensearch.core.compress.NoneCompressor;
import org.opensearch.index.snapshots.blobstore.BlobStoreIndexShardSnapshot;
import org.opensearch.index.store.StoreFileMetadata;
import org.opensearch.repositories.blobstore.BlobStoreRepository;
import org.opensearch.repositories.blobstore.ChecksumBlobStoreFormat;
import org.opensearch.snapshots.SnapshotId;
import org.opensearch.snapshots.SnapshotInfo;

/**
 * Writes a whole stock repository directory with the server's own formats:
 * one snapshot "snap-1" of one index "logs" of one shard, whose two files
 * (one stored in two parts, one whole) end in a real Lucene footer. The
 * files are not a Lucene index: they let the copy and its checks be tested
 * byte for byte.
 *
 * Usage: java GenRepo <out-dir>
 */
public class GenRepo {
    static final String UUID = "SnapUuidAaaaBbbbCccc01";
    static final String INDEX_ID = "IdxLogsAaaa1";
    static final String META_ID = "metaIdLogs1";

    public static void main(String[] a) throws Exception {
        Path out = Path.of(a[0]);
        Path shard = out.resolve("indices").resolve(INDEX_ID).resolve("0");
        Files.createDirectories(shard);

        Files.write(out.resolve("index.latest"), ByteBuffer.allocate(8).putLong(1L).array());
        String index = "{\"snapshots\":[{\"name\":\"snap-1\",\"uuid\":\"" + UUID + "\",\"state\":1,"
            + "\"index_metadata_lookup\":{\"" + INDEX_ID + "\":\"mdLogs1\"},\"version\":\"3.10.0\"}],"
            + "\"indices\":{\"logs\":{\"id\":\"" + INDEX_ID + "\",\"shard_path_type\":0,\"snapshots\":[\"" + UUID
            + "\"],\"shard_generations\":[\"gen0\"]}},\"index_metadata_identifiers\":{\"mdLogs1\":\"" + META_ID + "\"}}";
        Files.writeString(out.resolve("index-1"), index);

        SnapshotId id = new SnapshotId("snap-1", UUID);
        SnapshotInfo info = new SnapshotInfo(id, List.of("logs"), List.of(), 1_700_000_000_000L, null, 1_700_000_005_000L, 1, List.of(), false, Map.of(), false);
        write(out.resolve("snap-" + UUID + ".dat"), BlobStoreRepository.SNAPSHOT_FORMAT, info, "snap-" + UUID);

        IndexMetadata meta = IndexMetadata.builder("logs")
            .settings(Settings.builder()
                .put("index.version.created", org.opensearch.Version.CURRENT.id)
                .put("index.number_of_shards", 1)
                .put("index.number_of_replicas", 1)
                .put("index.uuid", INDEX_ID)
                .put("index.refresh_interval", "5s")
                .build())
            .putMapping("{\"_doc\":{\"properties\":{\"msg\":{\"type\":\"text\"},\"level\":{\"type\":\"keyword\"}}}}")
            .build();
        write(out.resolve("indices").resolve(INDEX_ID).resolve("meta-" + META_ID + ".dat"), BlobStoreRepository.INDEX_METADATA_FORMAT, meta, "meta-" + META_ID);

        byte[] big = file(3000, 7);
        byte[] small = file(100, 11);
        Files.write(shard.resolve("__bigBlobUuid1.part0"), java.util.Arrays.copyOfRange(big, 0, 2048));
        Files.write(shard.resolve("__bigBlobUuid1.part1"), java.util.Arrays.copyOfRange(big, 2048, big.length));
        Files.write(shard.resolve("__smallBlobUuid2"), small);
        StoreFileMetadata bigMeta = new StoreFileMetadata("_0.cfs", big.length, checksum(big), Version.LATEST);
        StoreFileMetadata smallMeta = new StoreFileMetadata("segments_1", small.length, checksum(small), Version.LATEST);
        BlobStoreIndexShardSnapshot snap = new BlobStoreIndexShardSnapshot(
            "snap-1", 1L,
            List.of(
                new BlobStoreIndexShardSnapshot.FileInfo("__bigBlobUuid1", bigMeta, new ByteSizeValue(2048L)),
                new BlobStoreIndexShardSnapshot.FileInfo("__smallBlobUuid2", smallMeta, null)),
            1_700_000_000_000L, 1234L, 2, (long) (big.length + small.length));
        write(shard.resolve("snap-" + UUID + ".dat"), BlobStoreRepository.INDEX_SHARD_SNAPSHOT_FORMAT, snap, "snap-" + UUID);
    }

    /** `len` bytes: a pattern, then a Lucene footer (magic, algorithm 0, CRC). */
    static byte[] file(int len, int seed) {
        ByteBuffer b = ByteBuffer.allocate(len);
        for (int i = 0; i < len - 16; i++) b.put((byte) (i * seed + 3));
        b.putInt(0xC02893E8).putInt(0);
        CRC32 crc = new CRC32();
        crc.update(b.array(), 0, len - 8);
        b.putLong(crc.getValue());
        return b.array();
    }

    static String checksum(byte[] file) {
        long crc = ByteBuffer.wrap(file, file.length - 8, 8).getLong();
        return Long.toString(crc, 36);
    }

    static <T extends org.opensearch.core.xcontent.ToXContent> void write(Path to, ChecksumBlobStoreFormat<T> format, T obj, String name) throws Exception {
        BytesReference bytes = format.serialize(obj, name, new NoneCompressor(), ChecksumBlobStoreFormat.SNAPSHOT_ONLY_FORMAT_PARAMS);
        Files.write(to, BytesReference.toBytes(bytes));
    }
}

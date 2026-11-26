import java.nio.file.*;
import java.util.*;
import org.apache.lucene.codecs.CodecUtil;
import org.apache.lucene.store.*;
import org.opensearch.core.xcontent.*;
import org.opensearch.common.xcontent.json.JsonXContent;
import org.opensearch.repositories.RepositoryData;
import org.opensearch.core.common.bytes.BytesArray;
import org.opensearch.core.common.bytes.BytesReference;
import org.opensearch.repositories.blobstore.BlobStoreRepository;
import org.opensearch.cluster.metadata.IndexMetadata;
import org.opensearch.cluster.metadata.Metadata;
import org.opensearch.snapshots.SnapshotInfo;
import org.opensearch.index.snapshots.blobstore.BlobStoreIndexShardSnapshot;
import org.opensearch.index.snapshots.blobstore.BlobStoreIndexShardSnapshots;

/**
 * Reads what the Rust writers left (OSS_WRITE_STOCK=dir cargo test -p
 * oss-format --test stock_envelope_write --test stock_repository_data_write):
 * each `<codec>.dat` through Lucene's CodecUtil (header at version 1, then
 * checksumEntireFile and retrieveChecksum), and each `index-N` through the
 * server's RepositoryData.snapshotsFromXContent. One line each; non-zero exit
 * on a refusal.
 */
public class CheckStockWrite {
    /** The server's own format over the whole blob, for the blobs that have one. */
    static String semantic(String name, byte[] bytes) throws Exception {
        BytesReference ref = new BytesArray(bytes);
        List<NamedXContentRegistry.Entry> entries = new ArrayList<>(org.opensearch.cluster.ClusterModule.getNamedXWriteables());
        entries.addAll(org.opensearch.indices.IndicesModule.getNamedXContents());
        NamedXContentRegistry reg = new NamedXContentRegistry(entries);
        if (name.startsWith("index-metadata")) {
            IndexMetadata m = BlobStoreRepository.INDEX_METADATA_FORMAT.deserialize(name, reg, ref);
            return " index=" + m.getIndex().getName() + " shards=" + m.getNumberOfShards()
                + " routing=" + m.getRoutingNumShards() + " mappings=" + m.mapping();
        }
        if (name.startsWith("global-meta")) {
            Metadata m = BlobStoreRepository.GLOBAL_METADATA_FORMAT.deserialize(name, reg, ref);
            return " clusterUUID=" + m.clusterUUID();
        }
        if (name.equals("snapshot-info.dat")) {
            SnapshotInfo i = BlobStoreRepository.SNAPSHOT_FORMAT.deserialize(name, reg, ref);
            return " snapshot=" + i.snapshotId() + " indices=" + i.indices() + " state=" + i.state();
        }
        if (name.equals("shard-snapshot.dat")) {
            BlobStoreIndexShardSnapshot s = BlobStoreRepository.INDEX_SHARD_SNAPSHOT_FORMAT.deserialize(name, reg, ref);
            return " files=" + s.indexFiles().size() + " totalSize=" + s.totalSize();
        }
        if (name.equals("shard-generation.dat")) {
            BlobStoreIndexShardSnapshots s = BlobStoreRepository.INDEX_SHARD_SNAPSHOTS_FORMAT.deserialize(name, reg, ref);
            return " snapshots=" + s.snapshots().size() + " files=" + s.iterator().hasNext();
        }
        return "";
    }

    public static void main(String[] a) throws Exception {
        int bad = 0;
        try (DirectoryStream<Path> files = Files.newDirectoryStream(Paths.get(a[0]))) {
            List<Path> sorted = new ArrayList<>();
            files.forEach(sorted::add);
            Collections.sort(sorted);
            for (Path f : sorted) {
                String name = f.getFileName().toString();
                byte[] bytes = Files.readAllBytes(f);
                try {
                    if (name.endsWith(".dat")) {
                        String codec = new String(bytes, 5, bytes[4], java.nio.charset.StandardCharsets.UTF_8);
                        try (Directory dir = new ByteBuffersDirectory()) {
                            try (IndexOutput o = dir.createOutput("x", IOContext.DEFAULT)) {
                                o.writeBytes(bytes, bytes.length);
                            }
                            try (ChecksumIndexInput in = dir.openChecksumInput("x")) {
                                CodecUtil.checkHeader(in, codec, 1, 1);
                                in.seek(bytes.length - CodecUtil.footerLength());
                                CodecUtil.checkFooter(in);
                            }
                            try (IndexInput in = dir.openInput("x", IOContext.READONCE)) {
                                CodecUtil.checksumEntireFile(in);
                                CodecUtil.retrieveChecksum(in);
                            }
                        }
                        System.out.println(name + ": ok" + semantic(name, bytes));
                    } else {
                        long gen = Long.parseLong(name.substring("index-".length()));
                        try (XContentParser p = JsonXContent.jsonXContent.createParser(
                                NamedXContentRegistry.EMPTY, DeprecationHandler.THROW_UNSUPPORTED_OPERATION, bytes)) {
                            RepositoryData d = RepositoryData.snapshotsFromXContent(p, gen);
                            System.out.println(name + ": ok snapshots=" + d.getSnapshotIds().size()
                                + " indices=" + d.getIndices().size());
                        }
                    }
                } catch (Throwable t) {
                    System.out.println(name + ": FAIL " + t);
                    bad++;
                }
            }
        }
        System.exit(bad == 0 ? 0 : 1);
    }
}

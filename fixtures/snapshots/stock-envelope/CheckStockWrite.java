import java.nio.file.*;
import java.util.*;
import org.apache.lucene.codecs.CodecUtil;
import org.apache.lucene.store.*;
import org.opensearch.core.xcontent.*;
import org.opensearch.common.xcontent.json.JsonXContent;
import org.opensearch.repositories.RepositoryData;

/**
 * Reads what the Rust writers left (OSS_WRITE_STOCK=dir cargo test -p
 * oss-format --test stock_envelope_write --test stock_repository_data_write):
 * each `<codec>.dat` through Lucene's CodecUtil (header at version 1, then
 * checksumEntireFile and retrieveChecksum), and each `index-N` through the
 * server's RepositoryData.snapshotsFromXContent. One line each; non-zero exit
 * on a refusal.
 */
public class CheckStockWrite {
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
                        String codec = name.startsWith("snapshot") ? "snapshot"
                            : name.substring(0, name.length() - 4);
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
                        System.out.println(name + ": ok");
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

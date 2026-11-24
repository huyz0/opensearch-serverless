import org.opensearch.serverless.cluster.IndexDescriptor;
import org.opensearch.serverless.store.CommitManifest;
import org.opensearch.serverless.store.FileLocation;
import java.io.ByteArrayInputStream;
import java.nio.file.*;
import java.util.*;

/** Reads what a Rust restore wrote for the index `copy`, as a Java node would: its descriptor and a shard's manifest. */
public class CheckRestore {
    static void require(boolean ok, String what) {
        if (!ok) {
            throw new AssertionError(what);
        }
        System.out.println("ok: " + what);
    }

    public static void main(String[] a) throws Exception {
        Path dir = Path.of(a[0]);
        final String uuid = Files.readString(dir.resolve("uuid.txt")).trim();
        final IndexDescriptor descriptor = IndexDescriptor.fromStream(new ByteArrayInputStream(Files.readAllBytes(dir.resolve("descriptor.bin"))));
        require(descriptor.name().equals("copy"), "the descriptor names the new index");
        require(descriptor.uuid().equals(uuid), "the descriptor holds a new uuid: " + descriptor.uuid());
        require(descriptor.numberOfShards() == 2, "the descriptor keeps the captured shard count");
        final CommitManifest commit = CommitManifest.fromStream(new ByteArrayInputStream(Files.readAllBytes(dir.resolve("manifest-1.bin"))));
        require(commit.term() == 1, "the restored shard starts at term 1");
        require(commit.writer() == null, "no writer owns the restored shard");
        require(commit.files().size() == 1, "one file is restored");
        for (String file : commit.files().keySet()) {
            final FileLocation location = commit.location(file);
            require(location != null && location.referencedBlob() != null, file + " resolves to " + location.referencedBlob());
            require(location.referencedBlob().contains("t=1"), file + " lies in the first-term container");
            require(commit.lengthOf(file) != null && commit.lengthOf(file) == 7L, file + " has its length");
        }
        System.out.println("PASS");
    }
}

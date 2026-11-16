package org.opensearch.serverless.membership;

import java.io.ByteArrayInputStream;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.HexFormat;
import java.util.List;
import java.util.Set;
import java.util.TreeSet;
import org.opensearch.core.common.bytes.BytesArray;
import org.opensearch.core.common.bytes.BytesReference;

/**
 * Reads each line of cases.txt and strict.txt with the shell's own
 * NodeLease.fromStream and writes toBytes of what it read, or "refused";
 * checks that each written lease reads back to the same bytes. Then encodes
 * the lease digests and members indexes of the fleets below with the shell's
 * own LeaseDigest, and the index as BlobLeaseMembership.encodeIds does.
 */
public class NodeLeases {
    static List<String> record(Path dir, String in) throws Exception {
        final List<String> lines = new ArrayList<>();
        for (String line : Files.readAllLines(dir.resolve(in), StandardCharsets.UTF_8)) {
            String out;
            try {
                final NodeLease lease = NodeLease.fromStream(new ByteArrayInputStream(line.getBytes(StandardCharsets.UTF_8)));
                out = lease.toBytes().utf8ToString();
                final NodeLease again = NodeLease.fromStream(new ByteArrayInputStream(out.getBytes(StandardCharsets.UTF_8)));
                if (again.toBytes().utf8ToString().equals(out) == false) {
                    throw new AssertionError("does not read back: " + out);
                }
            } catch (AssertionError e) {
                throw e;
            } catch (Exception e) {
                out = "refused";
            }
            lines.add(out);
        }
        return lines;
    }

    static NodeLease lease(String json) throws Exception {
        return NodeLease.fromStream(new ByteArrayInputStream(json.getBytes(StandardCharsets.UTF_8)));
    }

    /** BlobLeaseMembership.encodeIds, copied: the class needs the fork's register container to compile. */
    static BytesReference encodeIds(Set<String> ids) {
        return new BytesArray(String.join("\n", new TreeSet<>(ids)).getBytes(StandardCharsets.UTF_8));
    }

    public static void main(String[] args) throws Exception {
        final Path dir = Path.of(args[0]);
        final List<String> parsed = record(dir, "cases.txt");
        Files.write(dir.resolve("parsed.txt"), parsed, StandardCharsets.UTF_8);
        Files.write(dir.resolve("strict-parsed.txt"), record(dir, "strict.txt"), StandardCharsets.UTF_8);

        // Each fleet: the accepted cases at these lines (1-based), digested at this time.
        final int[][] fleets = { {}, { 1 }, { 1, 2, 3 }, { 2, 8, 12, 14, 24 } };
        final List<String> digests = new ArrayList<>();
        final List<String> indexes = new ArrayList<>();
        final HexFormat hex = HexFormat.of();
        for (int f = 0; f < fleets.length; f++) {
            final List<NodeLease> leases = new ArrayList<>();
            final Set<String> ids = new TreeSet<>();
            for (int line : fleets[f]) {
                final NodeLease lease = lease(parsed.get(line - 1));
                leases.add(lease);
                ids.add(lease.nodeId());
            }
            final long writtenAt = 1_700_000_000_000L + f;
            final BytesReference digest = LeaseDigest.encode(writtenAt, leases);
            final LeaseDigest back = LeaseDigest.decode(digest);
            if (back.writtenAtMillis() != writtenAt || back.leases().size() != leases.size()) {
                throw new AssertionError("digest does not read back");
            }
            digests.add(hex.formatHex(BytesReference.toBytes(digest)));
            indexes.add(hex.formatHex(BytesReference.toBytes(encodeIds(ids))));
        }
        Files.write(dir.resolve("digests.txt"), digests, StandardCharsets.UTF_8);
        Files.write(dir.resolve("indexes.txt"), indexes, StandardCharsets.UTF_8);
    }
}

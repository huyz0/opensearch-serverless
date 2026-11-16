import java.io.ByteArrayInputStream;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.List;
import org.opensearch.serverless.cluster.AliasRecord;

/**
 * Reads each line of cases.txt and strict.txt with the shell's own
 * AliasRecord.fromStream and writes what it reads: toBytes, then the write
 * index, the index writes go to ("-" for none), whether a creation and a
 * mutation are pending, and whether index "b"'s hint is protected; or
 * "refused".
 */
public class Aliases {
    static String show(String value) {
        return value == null ? "-" : value;
    }

    static void record(Path dir, String in, String out) throws Exception {
        final List<String> lines = new ArrayList<>();
        for (String line : Files.readAllLines(dir.resolve(in), StandardCharsets.UTF_8)) {
            try {
                final AliasRecord r = AliasRecord.fromStream(new ByteArrayInputStream(line.getBytes(StandardCharsets.UTF_8)));
                lines.add(r.toBytes().utf8ToString());
                lines.add(
                    show(r.writeIndex()) + "\t" + show(r.writesTo()) + "\t" + r.creationPending() + "\t" + r.mutationPending()
                        + "\t" + r.protectsHint("b")
                );
            } catch (Exception e) {
                lines.add("refused");
                lines.add("-");
            }
        }
        Files.write(dir.resolve(out), lines, StandardCharsets.UTF_8);
    }

    public static void main(String[] args) throws Exception {
        final Path dir = Path.of(args[0]);
        record(dir, "cases.txt", "parsed.txt");
        record(dir, "strict.txt", "strict-parsed.txt");
    }
}

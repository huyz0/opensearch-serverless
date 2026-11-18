import java.io.ByteArrayInputStream;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.List;
import org.opensearch.serverless.metadata.ShardHead;

/** Writes parsed.txt: for each case in cases.txt, ShardHead.toBytes of the head fromStream reads, or "refused". */
public final class ShardHeads {
    public static void main(String[] args) throws Exception {
        final Path dir = Path.of(args[0]);
        final List<String> out = new ArrayList<>();
        for (String line : Files.readAllLines(dir.resolve("cases.txt"))) {
            try {
                final ShardHead head = ShardHead.fromStream(new ByteArrayInputStream(line.getBytes(StandardCharsets.UTF_8)));
                out.add(new String(head.toBytes().toBytesRef().bytes, 0, head.toBytes().length(), StandardCharsets.UTF_8));
            } catch (Exception e) {
                out.add("refused");
            }
        }
        Files.write(dir.resolve("parsed.txt"), out);
    }
}

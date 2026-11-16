import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import org.opensearch.common.settings.Settings;
import org.opensearch.common.xcontent.XContentFactory;
import org.opensearch.common.xcontent.XContentType;
import org.opensearch.core.common.bytes.BytesReference;
import org.opensearch.core.xcontent.DeprecationHandler;
import org.opensearch.core.xcontent.NamedXContentRegistry;
import org.opensearch.core.xcontent.ToXContent;
import org.opensearch.core.xcontent.XContentBuilder;
import org.opensearch.core.xcontent.XContentParser;

/**
 * Writes the cluster-config fixtures with OpenSearch 3.3.0's own Settings.
 * parse, toBytes and the merge in update are ClusterConfig's, from
 * huyz0/OpenSearch serverless/shell/.../metadata/ClusterConfig.java at
 * 3920dc36, without the register.
 */
public class ClusterSettings {
    static Settings parse(byte[] in) throws IOException {
        try (
            XContentParser parser = XContentType.JSON.xContent()
                .createParser(NamedXContentRegistry.EMPTY, DeprecationHandler.THROW_UNSUPPORTED_OPERATION, in)
        ) {
            return Settings.fromXContent(parser);
        }
    }

    static String toBytes(Settings settings) throws IOException {
        try (XContentBuilder builder = XContentFactory.jsonBuilder()) {
            builder.startObject();
            settings.toXContent(builder, ToXContent.EMPTY_PARAMS);
            builder.endObject();
            return BytesReference.bytes(builder).utf8ToString();
        }
    }

    static String flat(Settings settings) throws IOException {
        try (XContentBuilder builder = XContentFactory.jsonBuilder()) {
            builder.startObject();
            settings.toXContent(builder, new ToXContent.MapParams(Map.of("flat_settings", "true")));
            builder.endObject();
            return BytesReference.bytes(builder).utf8ToString();
        }
    }

    static Settings update(Settings current, Map<String, Object> changes) {
        final Settings.Builder merging = Settings.builder().put(current);
        for (Map.Entry<String, Object> change : changes.entrySet()) {
            if (change.getValue() == null) {
                merging.putNull(change.getKey());
            } else {
                merging.put(change.getKey(), String.valueOf(change.getValue()));
            }
        }
        final Settings merged = merging.build();
        final Settings.Builder clean = Settings.builder();
        for (String key : merged.keySet()) {
            final String value = merged.get(key);
            if (value != null) {
                clean.put(key, value);
            }
        }
        return clean.build();
    }

    // The flat form read back must be the same settings: what a Java node
    // reads from what the Rust writes.
    static String checked(Settings settings) throws IOException {
        final String flat = flat(settings);
        final Settings again = parse(flat.getBytes(StandardCharsets.UTF_8));
        if (flat(again).equals(flat) == false) {
            throw new IllegalStateException("flat form does not read back: " + flat);
        }
        return flat;
    }

    public static void main(String[] args) throws IOException {
        final Path dir = Path.of(args[0]);
        final List<String> parsed = new ArrayList<>();
        for (String line : Files.readAllLines(dir.resolve("cases.txt"), StandardCharsets.UTF_8)) {
            try {
                parsed.add(checked(parse(line.getBytes(StandardCharsets.UTF_8))));
            } catch (RuntimeException | IOException e) {
                parsed.add("refused");
            }
        }
        Files.write(dir.resolve("parsed.txt"), parsed, StandardCharsets.UTF_8);

        final List<String> updated = new ArrayList<>();
        Settings current = Settings.EMPTY;
        for (String line : Files.readAllLines(dir.resolve("updates.txt"), StandardCharsets.UTF_8)) {
            // One change a word: `key=value` sets the key, `key` alone removes it.
            final Map<String, Object> changes = new LinkedHashMap<>();
            for (String word : line.split(" ")) {
                final int eq = word.indexOf('=');
                if (eq < 0) {
                    changes.put(word, null);
                } else {
                    changes.put(word.substring(0, eq), word.substring(eq + 1));
                }
            }
            current = update(current, changes);
            final String stored = toBytes(current);
            if (parse(stored.getBytes(StandardCharsets.UTF_8)).equals(current) == false) {
                throw new IllegalStateException("stored form does not read back: " + stored);
            }
            updated.add(stored);
            updated.add(checked(current));
        }
        Files.write(dir.resolve("updated.txt"), updated, StandardCharsets.UTF_8);
    }
}

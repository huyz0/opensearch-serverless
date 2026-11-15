// Writes the JSON this directory holds with Jackson 3, configured as
// OpenSearch's JsonXContent (libs/x-content JsonXContent.java at 3920dc36),
// field by field in the order IndexDescriptor.toBytes writes them.
// Run: java -cp jackson-core-3.2.3.jar Record.java <out-dir>
import java.io.ByteArrayOutputStream;
import java.nio.file.Files;
import java.nio.file.Path;

import tools.jackson.core.JsonGenerator;
import tools.jackson.core.ObjectWriteContext;
import tools.jackson.core.StreamWriteFeature;
import tools.jackson.core.json.JsonFactory;
import tools.jackson.core.json.JsonFactoryBuilder;
import tools.jackson.core.json.JsonWriteFeature;

public class Record {
    static final JsonFactory FACTORY;
    static {
        JsonFactoryBuilder b = new JsonFactoryBuilder(new JsonFactory());
        b.configure(JsonFactory.Feature.FAIL_ON_SYMBOL_HASH_OVERFLOW, false);
        b.configure(JsonWriteFeature.QUOTE_PROPERTY_NAMES, true);
        b.configure(StreamWriteFeature.AUTO_CLOSE_CONTENT, false);
        FACTORY = b.build();
    }

    interface Body { void write(JsonGenerator g); }

    static void write(Path dir, String name, Body body) throws Exception {
        ByteArrayOutputStream out = new ByteArrayOutputStream();
        try (JsonGenerator g = FACTORY.createGenerator(ObjectWriteContext.empty(), out)) {
            body.write(g);
        }
        Files.write(dir.resolve(name), out.toByteArray());
    }

    static String everyCharacter() {
        StringBuilder s = new StringBuilder();
        for (int c = 0; c < 0x80; c++) {
            s.append((char) c);
        }
        return s.append("\u0080\u009fé  😀﻿").toString();
    }

    static final String MAPPING =
        "{\"properties\":{\"path\":{\"type\":\"keyword\",\"null_value\":\"a/b\\\\c\"},\"téxt\":{\"type\":\"text\"}},\"_meta\":{\"note\":\"tab\\there\\nnew\"}}";

    public static void main(String[] args) throws Exception {
        Path dir = Path.of(args[0]);
        write(dir, "every-character.json", g -> g.writeString(everyCharacter()));
        write(dir, "descriptor-layout.json", g -> {
            g.writeStartObject();
            g.writeStringProperty("name", "logs-2026.11");
            g.writeStringProperty("uuid", "Zq3vX0yJT8mN1bW2cD4eFg");
            g.writeNumberProperty("number_of_shards", 2);
            g.writeStringProperty("mapping", MAPPING);
            g.writeNumberProperty("mapping_version", 3L);
            g.writeNumberProperty("settings_version", 7L);
            g.writeNumberProperty("created_at_millis", 1763164800123L);
            g.writeNumberProperty("created_version_id", 137217827);
            g.writeName("aliased_by");
            g.writeStartArray();
            g.writeString("logs");
            g.writeString("logs-été");
            g.writeEndArray();
            g.writeName("unit_layout");
            g.writeStartObject();
            g.writeStringProperty("mode", "adaptive");
            g.writeNumberProperty("declared_shards", 2);
            g.writeNumberProperty("next_unit_id", 2);
            g.writeName("units");
            g.writeStartArray();
            g.writeStartObject();
            g.writeNumberProperty("id", 0);
            g.writeStringProperty("first", "0");
            g.writeStringProperty("last", "7fffffffffffffff");
            g.writeEndObject();
            g.writeStartObject();
            g.writeNumberProperty("id", 1);
            g.writeStringProperty("first", "8000000000000000");
            g.writeStringProperty("last", "ffffffffffffffff");
            g.writeEndObject();
            g.writeEndArray();
            g.writeEndObject();
            g.writeName("settings");
            g.writeStartObject();
            g.writeName("index");
            g.writeStartObject();
            g.writeName("analysis");
            g.writeStartObject();
            g.writeName("filter");
            g.writeStartArray();
            g.writeString("icu_folding");
            g.writeEndArray();
            g.writeEndObject();
            g.writeStringProperty("refresh_interval", "1s");
            g.writeEndObject();
            g.writeEndObject();
            g.writeEndObject();
        });
        write(dir, "descriptor-bucket.json", g -> {
            g.writeStartObject();
            g.writeStringProperty("name", "tiny");
            g.writeStringProperty("uuid", "u-1");
            g.writeNumberProperty("number_of_shards", 1);
            g.writeNumberProperty("mapping_version", -1L);
            g.writeNumberProperty("created_at_millis", Long.MAX_VALUE);
            g.writeNumberProperty("created_version_id", Integer.MIN_VALUE);
            g.writeStringProperty("bucket", "tenant-7.b3");
            g.writeName("settings");
            g.writeStartObject();
            g.writeEndObject();
            g.writeEndObject();
        });
    }
}

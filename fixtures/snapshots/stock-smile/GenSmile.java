import java.io.*;
import java.math.*;
import java.nio.file.*;
import java.util.*;
import tools.jackson.core.*;
import tools.jackson.core.json.JsonFactory;
import tools.jackson.dataformat.smile.*;

/** Writes each case as SMILE (three encoder configurations) and as compact JSON. */
public class GenSmile {
    interface Case { void emit(JsonGenerator g) throws Exception; }

    static String repeat(String s, int n) { return s.repeat(n); }

    public static void main(String[] a) throws Exception {
        Path out = Paths.get(a[0]);
        Files.createDirectories(out);
        Map<String, Case> cases = new LinkedHashMap<>();
        cases.put("scalars", g -> {
            g.writeStartObject();
            g.writeName("n"); g.writeNull();
            g.writeName("t"); g.writeBoolean(true);
            g.writeName("f"); g.writeBoolean(false);
            g.writeName("e"); g.writeString("");
            g.writeName("s"); g.writeString("hello");
            g.writeEndObject();
        });
        cases.put("ints", g -> {
            g.writeStartArray();
            for (int v : new int[]{0, 1, -1, 15, 16, -16, -17, 63, 64, 127, 128, 1000, -1000, 65535, 1 << 20, Integer.MAX_VALUE, Integer.MIN_VALUE}) g.writeNumber(v);
            for (long v : new long[]{1L << 31, -(1L << 31) - 1, 1L << 40, Long.MAX_VALUE, Long.MIN_VALUE}) g.writeNumber(v);
            g.writeNumber(new BigInteger("123456789012345678901234567890"));
            g.writeNumber(new BigInteger("-98765432109876543210"));
            g.writeEndArray();
        });
        cases.put("floats", g -> {
            g.writeStartArray();
            g.writeNumber(1.5f); g.writeNumber(-0.25f); g.writeNumber(0.1f); g.writeNumber(3.4028235E38f);
            g.writeNumber(1.5d); g.writeNumber(-0.1d); g.writeNumber(1e300d); g.writeNumber(Double.MIN_VALUE); g.writeNumber(123456.789d);
            g.writeNumber(new BigDecimal("1.5")); g.writeNumber(new BigDecimal("-12345678901234567890.123456789")); g.writeNumber(new BigDecimal("0.000123"));
            g.writeEndArray();
        });
        cases.put("strings", g -> {
            g.writeStartArray();
            for (String s : new String[]{"a", "ab", repeat("x", 32), repeat("x", 33), repeat("x", 64), repeat("y", 65), repeat("z", 1000),
                "café", "日本語", "😀 emoji", repeat("é", 40), repeat("é", 100), "quote\" back\\ nl\n tab\t ctl\u0001"}) g.writeString(s);
            g.writeEndArray();
        });
        cases.put("binary", g -> {
            g.writeStartObject();
            g.writeName("empty"); g.writeBinary(new byte[0]);
            for (int n : new int[]{1, 2, 6, 7, 8, 13, 14, 15, 100, 4096}) {
                byte[] b = new byte[n];
                for (int i = 0; i < n; i++) b[i] = (byte) (i * 31 + 7);
                g.writeName("b" + n); g.writeBinary(b);
            }
            g.writeEndObject();
        });
        cases.put("nested", g -> {
            g.writeStartObject();
            g.writeName("a"); g.writeStartObject();
            g.writeName("b"); g.writeStartArray();
            g.writeStartObject(); g.writeName("c"); g.writeNumber(1); g.writeEndObject();
            g.writeStartArray(); g.writeEndArray();
            g.writeStartObject(); g.writeEndObject();
            g.writeEndArray();
            g.writeEndObject();
            g.writeName("a2"); g.writeString("v");
            g.writeEndObject();
        });
        cases.put("shared_names", g -> {
            g.writeStartArray();
            for (int i = 0; i < 200; i++) {
                g.writeStartObject();
                for (int k = 0; k < 40; k++) { g.writeName("field_" + k); g.writeNumber(i + k); }
                g.writeEndObject();
            }
            g.writeEndArray();
        });
        cases.put("name_table_overflow", g -> {
            g.writeStartArray();
            for (int r = 0; r < 3; r++) {
                g.writeStartObject();
                for (int k = 0; k < 1200; k++) { g.writeName("key" + k); g.writeNumber(k); }
                g.writeEndObject();
            }
            g.writeEndArray();
        });
        cases.put("long_names", g -> {
            g.writeStartObject();
            g.writeName(repeat("n", 64)); g.writeNumber(1);
            g.writeName(repeat("n", 65)); g.writeNumber(2);
            g.writeName(repeat("é", 70)); g.writeNumber(3);
            g.writeName(""); g.writeNumber(4);
            g.writeName("日本"); g.writeNumber(5);
            g.writeName(repeat("m", 63)); g.writeNumber(6);
            g.writeName(repeat("m", 63)); g.writeNumber(7);
            g.writeEndObject();
        });
        cases.put("shared_values", g -> {
            g.writeStartArray();
            for (int i = 0; i < 600; i++) { g.writeString("value_" + (i % 50)); g.writeString("v" + (i % 1200)); }
            g.writeEndArray();
        });
        cases.put("mapping_like", g -> {
            g.writeStartObject();
            g.writeName("_doc"); g.writeStartObject();
            g.writeName("properties"); g.writeStartObject();
            for (String f : new String[]{"title", "body", "tag", "ts"}) {
                g.writeName(f); g.writeStartObject();
                g.writeName("type"); g.writeString(f.equals("tag") ? "keyword" : f.equals("ts") ? "date" : "text");
                g.writeEndObject();
            }
            g.writeEndObject(); g.writeEndObject(); g.writeEndObject();
        });
        for (var e : cases.entrySet()) {
            // JSON
            StringWriter w = new StringWriter();
            try (JsonGenerator g = new JsonFactory().createGenerator(ObjectWriteContext.empty(), w)) { e.getValue().emit(g); }
            Files.writeString(out.resolve(e.getKey() + ".json"), w.toString());
            // SMILE: names only; names and values; names, values and 7-bit binary
            for (int mode = 0; mode < 3; mode++) {
                SmileFactoryBuilder b = SmileFactory.builder();
                if (mode >= 1) b.enable(SmileWriteFeature.CHECK_SHARED_STRING_VALUES);
                if (mode >= 2) b.enable(SmileWriteFeature.ENCODE_BINARY_AS_7BIT);
                if (mode == 0) b.disable(SmileWriteFeature.ENCODE_BINARY_AS_7BIT);
                SmileFactory f = b.build();
                ByteArrayOutputStream bo = new ByteArrayOutputStream();
                try (JsonGenerator g = f.createGenerator(ObjectWriteContext.empty(), bo)) { e.getValue().emit(g); }
                Files.write(out.resolve(e.getKey() + ".m" + mode + ".smile"), bo.toByteArray());
            }
        }
    }
}

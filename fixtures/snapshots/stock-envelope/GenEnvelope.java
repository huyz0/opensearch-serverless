import java.io.ByteArrayOutputStream;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.zip.Deflater;
import java.util.zip.DeflaterOutputStream;
import org.apache.lucene.codecs.CodecUtil;
import org.apache.lucene.store.ByteBuffersDataOutput;
import org.apache.lucene.store.ByteBuffersIndexOutput;
import tools.jackson.dataformat.smile.SmileFactory;
import tools.jackson.core.JsonGenerator;

/**
 * Writes the envelope a stock OpenSearch repository puts round every metadata
 * blob (ChecksumBlobStoreFormat): Lucene's CodecUtil header with the codec
 * name and version 1, a SMILE body (optionally the DFL deflate framing of
 * DeflateCompressor), and CodecUtil's footer. Cases: plain and compressed.
 *
 * Usage: java GenEnvelope <out-dir>
 */
public class GenEnvelope {
    static byte[] smile() throws Exception {
        SmileFactory f = new SmileFactory();
        ByteArrayOutputStream bos = new ByteArrayOutputStream();
        try (JsonGenerator g = f.createGenerator(bos)) {
            g.writeStartObject();
            g.writeStringProperty("uuid", "abc-DEF_123");
            g.writeNumberProperty("version", 7);
            g.writeName("files");
            g.writeStartArray();
            for (int i = 0; i < 3; i++) {
                g.writeStartObject();
                g.writeStringProperty("name", "_" + i + ".cfs");
                g.writeNumberProperty("length", 1000L * (i + 1));
                g.writeEndObject();
            }
            g.writeEndArray();
            g.writeEndObject();
        }
        return bos.toByteArray();
    }

    static byte[] envelope(String codec, byte[] body) throws Exception {
        ByteBuffersDataOutput buf = new ByteBuffersDataOutput();
        try (ByteBuffersIndexOutput out = new ByteBuffersIndexOutput(buf, "blob", "blob")) {
            CodecUtil.writeHeader(out, codec, 1);
            out.writeBytes(body, body.length);
            CodecUtil.writeFooter(out);
        }
        return buf.toArrayCopy();
    }

    static byte[] deflate(byte[] raw) throws Exception {
        ByteArrayOutputStream bos = new ByteArrayOutputStream();
        bos.write(new byte[] {'D', 'F', 'L', 0});
        Deflater d = new Deflater(Deflater.DEFAULT_COMPRESSION, true);
        try (DeflaterOutputStream o = new DeflaterOutputStream(bos, d)) {
            o.write(raw);
        }
        return bos.toByteArray();
    }

    public static void main(String[] a) throws Exception {
        Path dir = Path.of(a[0]);
        byte[] body = smile();
        Files.write(dir.resolve("snapshot.plain.dat"), envelope("snapshot", body));
        Files.write(dir.resolve("snapshot.deflate.dat"), envelope("snapshot", deflate(body)));
        Files.write(dir.resolve("meta.plain.dat"), envelope("metadata", body));
        Files.write(dir.resolve("body.smile"), body);
    }
}

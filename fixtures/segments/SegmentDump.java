import java.nio.charset.CharacterCodingException;
import java.nio.charset.CodingErrorAction;
import java.nio.charset.StandardCharsets;
import java.nio.ByteBuffer;
import java.nio.file.Paths;
import java.util.ArrayList;
import java.util.List;
import java.util.Map;
import java.util.TreeMap;

import org.apache.lucene.index.*;
import org.apache.lucene.search.DocIdSetIterator;
import org.apache.lucene.store.Directory;
import org.apache.lucene.store.FSDirectory;
import org.apache.lucene.util.BytesRef;

/**
 * Prints a Lucene index's one segment as JSON: its field infos, and for each
 * document its stored fields, inverted terms (freq, positions, offsets),
 * norms, doc values and points. record_segments.py runs it on the segment
 * stock OpenSearch wrote; the writer's tests build the same dump from what
 * they hand the engine. Needs only lucene-core on the class path.
 *
 *     java -cp lucene-core.jar SegmentDump.java <index directory>
 *
 * Bytes are written as a string when they are valid UTF-8 with no control
 * character, otherwise as "hex:" and their hex. Values sorted by the codec
 * (sorted-numeric, sorted-set, points) are written in sorted order.
 */
public class SegmentDump {
    public static void main(String[] args) throws Exception {
        try (Directory dir = FSDirectory.open(Paths.get(args[0])); DirectoryReader reader = DirectoryReader.open(dir)) {
            if (reader.leaves().size() != 1) {
                throw new IllegalStateException("expected one segment, found " + reader.leaves().size());
            }
            LeafReader leaf = reader.leaves().get(0).reader();
            StringBuilder out = new StringBuilder("{\n  \"max_doc\": ").append(leaf.maxDoc()).append(",\n  \"fields\": [");
            boolean first = true;
            for (FieldInfo f : leaf.getFieldInfos()) {
                out.append(first ? "\n" : ",\n").append("    {\"number\": ").append(f.number)
                    .append(", \"name\": ").append(str(f.name))
                    .append(", \"index_options\": ").append(str(f.getIndexOptions().toString()))
                    .append(", \"doc_values\": ").append(str(f.getDocValuesType().toString()))
                    .append(", \"skip_index\": ").append(str(f.docValuesSkipIndexType().toString()))
                    .append(", \"omit_norms\": ").append(f.omitsNorms())
                    .append(", \"points\": [").append(f.getPointDimensionCount()).append(", ")
                    .append(f.getPointIndexDimensionCount()).append(", ").append(f.getPointNumBytes()).append("]")
                    .append(", \"payloads\": ").append(f.hasPayloads())
                    .append(", \"term_vectors\": ").append(f.hasTermVectors()).append("}");
                first = false;
            }
            out.append("\n  ],\n  \"docs\": [");
            for (int doc = 0; doc < leaf.maxDoc(); doc++) {
                out.append(doc == 0 ? "\n" : ",\n").append(document(leaf, doc));
            }
            out.append("\n  ]\n}\n");
            System.out.print(out);
        }
    }

    static String document(LeafReader leaf, int doc) throws Exception {
        List<String> stored = new ArrayList<>();
        leaf.storedFields().document(doc, new StoredFieldVisitor() {
            public Status needsField(FieldInfo f) { return Status.YES; }
            public void binaryField(FieldInfo f, byte[] v) { stored.add(entry(f.name, "binary", bytes(new BytesRef(v)))); }
            public void stringField(FieldInfo f, String v) { stored.add(entry(f.name, "string", str(v))); }
            public void intField(FieldInfo f, int v) { stored.add(entry(f.name, "int", Integer.toString(v))); }
            public void longField(FieldInfo f, long v) { stored.add(entry(f.name, "long", Long.toString(v))); }
            public void floatField(FieldInfo f, float v) { stored.add(entry(f.name, "float", str(Float.toString(v)))); }
            public void doubleField(FieldInfo f, double v) { stored.add(entry(f.name, "double", str(Double.toString(v)))); }
        });
        Map<String, String> inverted = new TreeMap<>();
        Map<String, String> docValues = new TreeMap<>();
        Map<String, String> points = new TreeMap<>();
        for (FieldInfo f : leaf.getFieldInfos()) {
            if (f.getIndexOptions() != IndexOptions.NONE) {
                String terms = terms(leaf, f, doc);
                if (terms != null) {
                    NumericDocValues norms = leaf.getNormValues(f.name);
                    String norm = norms != null && norms.advanceExact(doc) ? Long.toString(norms.longValue()) : "null";
                    inverted.put(f.name, "{\"norm\": " + norm + ", \"terms\": " + terms + "}");
                }
            }
            String dv = docValues(leaf, f, doc);
            if (dv != null) {
                docValues.put(f.name, dv);
            }
            if (f.getPointDimensionCount() > 0) {
                List<String> values = new ArrayList<>();
                leaf.getPointValues(f.name).intersect(new PointValues.IntersectVisitor() {
                    public void visit(int d) { throw new IllegalStateException("inside"); }
                    public void visit(int d, byte[] packed) { if (d == doc) values.add(bytes(new BytesRef(packed))); }
                    public PointValues.Relation compare(byte[] min, byte[] max) { return PointValues.Relation.CELL_CROSSES_QUERY; }
                });
                if (!values.isEmpty()) {
                    values.sort(null);
                    points.put(f.name, "[" + String.join(", ", values) + "]");
                }
            }
        }
        return "    {\"stored\": [" + String.join(", ", stored) + "],\n     \"inverted\": " + map(inverted)
            + ",\n     \"doc_values\": " + map(docValues) + ",\n     \"points\": " + map(points) + "}";
    }

    static String terms(LeafReader leaf, FieldInfo f, int doc) throws Exception {
        Terms terms = leaf.terms(f.name);
        if (terms == null) {
            return null;
        }
        boolean positions = f.getIndexOptions().compareTo(IndexOptions.DOCS_AND_FREQS_AND_POSITIONS) >= 0;
        boolean offsets = f.getIndexOptions().compareTo(IndexOptions.DOCS_AND_FREQS_AND_POSITIONS_AND_OFFSETS) >= 0;
        List<String> out = new ArrayList<>();
        TermsEnum te = terms.iterator();
        for (BytesRef term = te.next(); term != null; term = te.next()) {
            PostingsEnum p = te.postings(null, PostingsEnum.ALL);
            if (p.advance(doc) != doc) {
                continue;
            }
            StringBuilder s = new StringBuilder("[").append(bytes(term)).append(", ").append(p.freq());
            if (positions) {
                StringBuilder pos = new StringBuilder();
                StringBuilder off = new StringBuilder();
                for (int i = 0; i < p.freq(); i++) {
                    pos.append(i == 0 ? "" : ", ").append(p.nextPosition());
                    if (offsets) {
                        off.append(i == 0 ? "" : ", ").append("[").append(p.startOffset()).append(", ").append(p.endOffset()).append("]");
                    }
                }
                s.append(", [").append(pos).append("]");
                if (offsets) {
                    s.append(", [").append(off).append("]");
                }
            }
            out.add(s.append("]").toString());
        }
        return out.isEmpty() ? null : "[" + String.join(", ", out) + "]";
    }

    static String docValues(LeafReader leaf, FieldInfo f, int doc) throws Exception {
        switch (f.getDocValuesType()) {
            case NUMERIC: {
                NumericDocValues v = leaf.getNumericDocValues(f.name);
                return v.advanceExact(doc) ? Long.toString(v.longValue()) : null;
            }
            case SORTED_NUMERIC: {
                SortedNumericDocValues v = leaf.getSortedNumericDocValues(f.name);
                if (!v.advanceExact(doc)) {
                    return null;
                }
                List<String> values = new ArrayList<>();
                for (int i = 0; i < v.docValueCount(); i++) {
                    values.add(Long.toString(v.nextValue()));
                }
                return "[" + String.join(", ", values) + "]";
            }
            case BINARY: {
                BinaryDocValues v = leaf.getBinaryDocValues(f.name);
                return v.advanceExact(doc) ? bytes(v.binaryValue()) : null;
            }
            case SORTED: {
                SortedDocValues v = leaf.getSortedDocValues(f.name);
                return v.advanceExact(doc) ? bytes(v.lookupOrd(v.ordValue())) : null;
            }
            case SORTED_SET: {
                SortedSetDocValues v = leaf.getSortedSetDocValues(f.name);
                if (!v.advanceExact(doc)) {
                    return null;
                }
                List<String> values = new ArrayList<>();
                for (int i = 0; i < v.docValueCount(); i++) {
                    values.add(bytes(v.lookupOrd(v.nextOrd())));
                }
                return "[" + String.join(", ", values) + "]";
            }
            default:
                return null;
        }
    }

    static String entry(String name, String type, String value) {
        return "[" + str(name) + ", " + str(type) + ", " + value + "]";
    }

    static String map(Map<String, String> m) {
        if (m.isEmpty()) {
            return "{}";
        }
        StringBuilder s = new StringBuilder("{");
        boolean first = true;
        for (Map.Entry<String, String> e : m.entrySet()) {
            s.append(first ? "" : ", ").append(str(e.getKey())).append(": ").append(e.getValue());
            first = false;
        }
        return s.append("}").toString();
    }

    static String bytes(BytesRef b) {
        try {
            String s = StandardCharsets.UTF_8.newDecoder()
                .onMalformedInput(CodingErrorAction.REPORT)
                .onUnmappableCharacter(CodingErrorAction.REPORT)
                .decode(ByteBuffer.wrap(b.bytes, b.offset, b.length))
                .toString();
            if (s.chars().noneMatch(c -> c < 0x20 || c == 0x7f) && !s.startsWith("hex:")) {
                return str(s);
            }
        } catch (CharacterCodingException e) {
            // not text: written as hex below
        }
        StringBuilder h = new StringBuilder("\"hex:");
        for (int i = 0; i < b.length; i++) {
            h.append(String.format("%02x", b.bytes[b.offset + i] & 0xff));
        }
        return h.append("\"").toString();
    }

    static String str(String s) {
        StringBuilder out = new StringBuilder("\"");
        for (char c : s.toCharArray()) {
            switch (c) {
                case '"': out.append("\\\""); break;
                case '\\': out.append("\\\\"); break;
                default:
                    if (c < 0x20 || c == 0x7f) {
                        out.append(String.format("\\u%04x", (int) c));
                    } else {
                        out.append(c);
                    }
            }
        }
        return out.append("\"").toString();
    }
}

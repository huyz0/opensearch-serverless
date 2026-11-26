import java.io.*;
import java.nio.file.*;
import java.util.*;
import tools.jackson.core.*;
import tools.jackson.core.json.JsonFactory;
import tools.jackson.dataformat.smile.*;

/**
 * Reads each `<name>.rust.smile` the Rust encoder wrote (OSS_WRITE_SMILE=dir
 * cargo test -p oss-format --test smile_write) with Jackson's SmileFactory
 * and compares the tree it reads with `<name>.json`, both printed as compact
 * JSON. Prints one line a case and exits non-zero on a difference.
 */
public class CheckSmileWrite {
    static String text(JsonParser p) throws Exception {
        StringWriter w = new StringWriter();
        try (JsonGenerator g = new JsonFactory().createGenerator(w)) {
            p.nextToken();
            g.copyCurrentStructure(p);
        }
        return w.toString();
    }

    public static void main(String[] a) throws Exception {
        Path written = Paths.get(a[0]);
        Path fixtures = Paths.get(a[1]);
        int bad = 0, n = 0;
        try (var files = Files.list(written)) {
            for (Path f : (Iterable<Path>) files.sorted()::iterator) {
                String name = f.getFileName().toString();
                if (!name.endsWith(".rust.smile")) continue;
                String stem = name.substring(0, name.length() - ".rust.smile".length());
                String got;
                try (JsonParser p = new SmileFactory().createParser(Files.readAllBytes(f))) {
                    got = text(p);
                }
                String want;
                try (JsonParser p = new JsonFactory().createParser(Files.readString(fixtures.resolve(stem + ".json")))) {
                    want = text(p);
                }
                boolean same = got.equals(want);
                System.out.println((same ? "ok " : "DIFF ") + stem);
                if (!same) { bad++; System.out.println("  got  " + got); System.out.println("  want " + want); }
                n++;
            }
        }
        System.out.println(n + " cases, " + bad + " different");
        if (bad > 0 || n == 0) System.exit(1);
    }
}

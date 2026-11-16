package org.opensearch.serverless.metadata;

import java.io.IOException;
import java.io.PrintWriter;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.HexFormat;
import java.util.List;

/**
 * Writes the transport-secret fixtures with the shell's own TransportSecrets
 * (huyz0/OpenSearch serverless/shell/.../metadata/TransportSecrets.java at
 * 429b4cbc): parse over cases.txt, and toJson over a few values.
 */
public class TransportSecretRecordings {
    static String hex(String s) {
        return s == null ? "~" : s.isEmpty() ? "-" : HexFormat.of().formatHex(s.getBytes(StandardCharsets.UTF_8));
    }

    public static void main(String[] args) throws IOException {
        final Path dir = Path.of(args[0]);
        final List<String> cases = Files.readAllLines(dir.resolve("cases.txt"), StandardCharsets.UTF_8);
        try (PrintWriter out = new PrintWriter(Files.newBufferedWriter(dir.resolve("parsed.txt"), StandardCharsets.UTF_8))) {
            for (String c : cases) {
                try {
                    final TransportSecrets s = TransportSecrets.parse(c);
                    out.println(s.generation() + " " + hex(s.current()) + " " + hex(s.previous()) + " " + s.rotatedAtMillis());
                } catch (IOException refused) {
                    out.println("refused");
                }
            }
        }
        final List<TransportSecrets> values = List.of(
            new TransportSecrets(1L, "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef", null, 1700000000000L),
            new TransportSecrets(42L, "aa11", "bb22", 1700000000123L),
            new TransportSecrets(-3L, "quote\"back\\slashé", "x", -1L)
        );
        try (PrintWriter out = new PrintWriter(Files.newBufferedWriter(dir.resolve("written.txt"), StandardCharsets.UTF_8))) {
            for (TransportSecrets v : values) {
                final String json = v.toJson();
                final TransportSecrets back = TransportSecrets.parse(json);
                if (back.equals(v) == false) {
                    throw new AssertionError("does not read back: " + json);
                }
                out.println(v.generation() + " " + hex(v.current()) + " " + hex(v.previous()) + " " + v.rotatedAtMillis() + " " + json);
            }
        }
    }
}

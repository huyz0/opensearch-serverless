// Writes the change-stripe fixtures with the Java's own code: State's add and
// encode, and stripeOf, copied verbatim from
// serverless/shell/src/main/java/org/opensearch/serverless/metadata/ChangeStripes.java@3920dc36,
// since running ChangeStripes itself needs the whole shell build.
//
//   java Stripes.java fixtures/change-stripes

import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.List;

public class Stripes {
    static final int STRIPES = 16;
    static final int KEPT = 256;
    static final int KEPT_PENDING = 64;

    enum Kind { PENDING_DELETE, DELETED, CHANGED }

    record Entry(long generation, boolean pending, String name) {}

    static int stripeOf(String name) {
        return Math.floorMod(name.hashCode(), STRIPES);
    }

    static final class State {
        private long epoch;
        private long generation;
        private final List<Entry> entries = new ArrayList<>();

        static State parse(String text) {
            final State state = new State();
            final String[] lines = text.split("\n");
            final String[] head = lines[0].split(" ");
            if (head.length != 3 || head[0].equals("v1") == false) {
                throw new IllegalArgumentException("not a change stripe: " + lines[0]);
            }
            state.epoch = Long.parseLong(head[1]);
            state.generation = Long.parseLong(head[2]);
            for (int i = 1; i < lines.length; i++) {
                if (lines[i].isEmpty()) {
                    continue;
                }
                final String[] parts = lines[i].split(" ", 3);
                state.entries.add(new Entry(Long.parseLong(parts[0]), parts[1].equals("P"), parts[2]));
            }
            return state;
        }

        String encode() {
            final StringBuilder out = new StringBuilder("v1 ").append(epoch).append(' ').append(generation).append('\n');
            for (Entry entry : entries) {
                out.append(entry.generation()).append(entry.pending() ? " P " : " D ").append(entry.name()).append('\n');
            }
            return out.toString();
        }

        void add(String name, Kind kind) {
            generation++;
            if (kind == Kind.DELETED) {
                entries.removeIf(entry -> entry.pending() && entry.name().equals(name));
            }
            entries.add(new Entry(generation, kind == Kind.PENDING_DELETE, name));
            int applied = 0;
            int pendingCount = 0;
            for (Entry entry : entries) {
                if (entry.pending()) {
                    pendingCount++;
                } else {
                    applied++;
                }
            }
            if (pendingCount > KEPT_PENDING) {
                epoch++;
                entries.clear();
                return;
            }
            for (var it = entries.iterator(); it.hasNext() && applied > KEPT;) {
                final Entry entry = it.next();
                if (entry.pending() == false) {
                    it.remove();
                    applied--;
                }
            }
        }
    }

    static void write(Path dir, String file, String text) throws Exception {
        Files.write(dir.resolve(file), text.getBytes(StandardCharsets.UTF_8));
    }

    public static void main(String[] args) throws Exception {
        final Path dir = Path.of(args[0]);

        // A delete's two bumps around an update, and a finished delete.
        State bumps = new State();
        bumps.add("alpha", Kind.CHANGED);
        bumps.add("beta", Kind.PENDING_DELETE);
        bumps.add("beta", Kind.CHANGED);
        bumps.add("gamma été 日本", Kind.CHANGED);
        bumps.add("alpha", Kind.PENDING_DELETE);
        bumps.add("beta", Kind.DELETED);
        write(dir, "bumps.txt", bumps.encode());

        // Past KEPT applied changes, with an unfinished delete among the oldest.
        State trimmed = new State();
        trimmed.add("n-0", Kind.CHANGED);
        trimmed.add("kept", Kind.PENDING_DELETE);
        for (int i = 1; i < 300; i++) {
            trimmed.add("n-" + i, Kind.CHANGED);
        }
        write(dir, "trimmed.txt", trimmed.encode());

        // One unfinished delete past KEPT_PENDING moves the epoch on.
        State epoch = new State();
        for (int i = 0; i <= KEPT_PENDING; i++) {
            epoch.add("gone-" + i, Kind.PENDING_DELETE);
        }
        epoch.add("after", Kind.CHANGED);
        write(dir, "new-epoch.txt", epoch.encode());

        // Parse and encode again what the Java reads leniently.
        write(dir, "reencoded.txt", State.parse("v1 3 9 \n\n5 D a b\n7 X c\n9 P d\n\n").encode());

        final StringBuilder of = new StringBuilder();
        for (String name : new String[] { "", "a", "alpha", "other-0", "logs-2026.11.16", "été", "日本", "😀x", "zzzzzzzzzzzz" }) {
            of.append(stripeOf(name)).append(' ').append(name).append('\n');
        }
        write(dir, "stripe-of.txt", of.toString());
    }
}

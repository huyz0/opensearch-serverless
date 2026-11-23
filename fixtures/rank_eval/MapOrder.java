import java.util.*;
import java.util.concurrent.ConcurrentHashMap;

/** Prints the iteration order of HashMap(n) and ConcurrentHashMap(n) after putting keys, one case a line. */
public class MapOrder {
    public static void main(String[] a) {
        Random r = new Random(7);
        List<String> pool = new ArrayList<>(List.of("q1", "q2", "q3", "alpha", "beta", "gamma", "delta", "title", "ünï", "a", "b", "c", "d", "e", "f"));
        for (int i = 0; i < 60; i++) pool.add("query-" + i);
        for (int n = 1; n <= 40; n++) {
            for (int rep = 0; rep < 3; rep++) {
                List<String> keys = new ArrayList<>(pool);
                Collections.shuffle(keys, r);
                // distinct keys, at most n of them, in the order put
                keys = keys.subList(0, rep == 0 ? n : Math.max(1, n - rep));
                Map<String, Integer> hm = new HashMap<>(n);
                Map<String, Integer> chm = new ConcurrentHashMap<>(n);
                for (String k : keys) { hm.put(k, 1); chm.put(k, 1); }
                System.out.println(n + "|" + String.join(",", keys) + "|" + String.join(",", hm.keySet()) + "|" + String.join(",", chm.keySet()));
            }
        }
    }
}

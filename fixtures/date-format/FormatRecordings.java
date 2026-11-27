import org.opensearch.common.time.DateFormatter;

import java.time.Instant;
import java.time.ZoneId;

/**
 * Prints what OpenSearch 3.10.0's {@code DateFormatter} writes for a pattern, an instant and a zone, one
 * {@code pattern|offset-seconds|millis|text} line each. Run from the OpenSearch 3.10.0 jars:
 * {@code java FormatRecordings.java > java-3.10.0.txt}.
 */
public final class FormatRecordings {
    public static void main(String[] a) {
        String[] patterns = {
            "strict_date_optional_time", "epoch_millis", "epoch_second", "strict_date", "date", "basic_date",
            "strict_year_month", "strict_year", "strict_hour_minute_second", "yyyy-MM-dd", "yyyy/MM/dd HH:mm:ss",
            "yyyy-MM-dd'T'HH:mm:ss.SSS", "dd.MM.yy", "d/M/yyyy H:m:s", "yyyyMMddHHmmssSSS", "yyyy-MM-dd HH:mm:ss.SS",
        };
        long[] millis = {0L, 1_700_000_000_123L, 951_782_400_000L, -86_400_001L, 253_402_300_799_999L, 1_000_000L};
        int[] offsets = {0, 3600, -18000};
        for (String p : patterns) {
            DateFormatter f = DateFormatter.forPattern(p);
            for (int off : offsets) {
                for (long m : millis) {
                    String text;
                    try {
                        text = f.withZone(java.time.ZoneOffset.ofTotalSeconds(off)).format(Instant.ofEpochMilli(m));
                    } catch (Exception e) {
                        text = "ERR " + e.getClass().getSimpleName();
                    }
                    System.out.println(p + "|" + off + "|" + m + "|" + text);
                }
            }
        }
    }
}

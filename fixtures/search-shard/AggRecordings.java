import org.opensearch.common.io.stream.BytesStreamOutput;
import org.opensearch.search.DocValueFormat;
import org.opensearch.search.aggregations.AggregatorFactories;
import org.opensearch.search.aggregations.AggregationBuilders;
import org.opensearch.search.aggregations.InternalAggregation;
import org.opensearch.search.aggregations.InternalAggregations;
import org.opensearch.search.aggregations.metrics.*;

import java.time.ZoneOffset;
import java.util.HexFormat;
import java.util.List;

/**
 * Writes the bytes OpenSearch 3.10.0's own classes put on the wire for a forwarded search's
 * aggregations (an {@code AggregatorFactories.Builder} and the shard's {@code InternalAggregations}),
 * one {@code name hex} line each: compiled against the 3.10.0 jars from Maven Central
 * ({@code opensearch}, its dependencies, {@code lucene-core} 10.6.0, jackson 2.21, snakeyaml 2.4),
 * {@code java AggRecordings > java-aggs-3.10.0.txt}.
 */
public final class AggRecordings {
    static void line(String name, org.opensearch.common.io.stream.BytesStreamOutput out) {
        System.out.println(name + " " + HexFormat.of().formatHex(java.util.Arrays.copyOf(out.bytes().toBytesRef().bytes, out.size())));
    }
    public static void main(String[] a) throws Exception {
        AggregatorFactories.Builder b = new AggregatorFactories.Builder()
            .addAggregator(AggregationBuilders.max("hi").field("n"))
            .addAggregator(AggregationBuilders.count("c").field("tag"))
            .addAggregator(AggregationBuilders.min("lo").field("n"))
            .addAggregator(AggregationBuilders.sum("s").field("n"))
            .addAggregator(AggregationBuilders.avg("av").field("n"))
            .addAggregator(AggregationBuilders.stats("st").field("n"));
        BytesStreamOutput out = new BytesStreamOutput();
        b.writeTo(out);
        line("request.metrics", out);

        DocValueFormat date = new DocValueFormat.DateTime(org.opensearch.index.mapper.DateFieldMapper.DEFAULT_DATE_TIME_FORMATTER, ZoneOffset.UTC, org.opensearch.index.mapper.DateFieldMapper.Resolution.MILLISECONDS);
        List<InternalAggregation> l = List.of(
            new InternalMax("hi", 2.0, DocValueFormat.RAW, null),
            new InternalValueCount("c", 7, null),
            new InternalMin("lo", 1.0, DocValueFormat.RAW, null),
            new InternalSum("s", 3.5, DocValueFormat.RAW, null),
            new InternalAvg("av", 6.0, 3, DocValueFormat.RAW, null),
            new InternalStats("st", 3, 6.0, 1.0, 4.0, DocValueFormat.RAW, null));
        out = new BytesStreamOutput();
        InternalAggregations.from(l).writeTo(out);
        line("response.metrics", out);
        out = new BytesStreamOutput();
        InternalAggregations.from(List.of(new InternalMax("t", 1.0, date, null))).writeTo(out);
        line("response.date_max", out);
    }
}

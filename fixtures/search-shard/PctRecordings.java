package org.opensearch.search.aggregations.metrics;

import org.opensearch.common.io.stream.BytesStreamOutput;
import org.opensearch.search.DocValueFormat;
import org.opensearch.search.aggregations.AggregationBuilders;
import org.opensearch.search.aggregations.AggregatorFactories;
import org.opensearch.search.aggregations.InternalAggregation;
import org.opensearch.search.aggregations.InternalAggregations;

import java.util.HexFormat;
import java.util.List;

/**
 * Writes the bytes OpenSearch 3.10.0 puts on the wire for forwarded {@code percentiles} and
 * {@code percentile_ranks} aggregations (the builders, and the shard's t-digest answers: ten values, five
 * hundred, no values, and ranks), one {@code name hex} line each. Compiled as {@code CardRecordings.java}:
 * {@code java org.opensearch.search.aggregations.metrics.PctRecordings > java-percentiles-3.10.0.txt}.
 */
public final class PctRecordings {
    static void line(String name, BytesStreamOutput out) {
        System.out.println(name + " " + HexFormat.of().formatHex(java.util.Arrays.copyOf(out.bytes().toBytesRef().bytes, out.size())));
    }
    static void answer(String name, InternalAggregation agg) throws Exception {
        BytesStreamOutput out = new BytesStreamOutput();
        InternalAggregations.from(List.of(agg)).writeTo(out);
        line(name, out);
    }
    public static void main(String[] a) throws Exception {
        BytesStreamOutput out = new BytesStreamOutput();
        new AggregatorFactories.Builder()
            .addAggregator(AggregationBuilders.percentiles("p").field("n"))
            .addAggregator(AggregationBuilders.percentiles("q").field("n").percentiles(50, 99.5).compression(200).keyed(false))
            .addAggregator(AggregationBuilders.percentileRanks("r", new double[] {10, 20}).field("n"))
            .writeTo(out);
        line("request.percentiles", out);
        TDigestState small = new TDigestState(100);
        for (int i = 1; i <= 10; i++) small.add(i);
        answer("response.percentiles_small", new InternalTDigestPercentiles("p", new double[] {1, 5, 25, 50, 75, 95, 99}, small, true, DocValueFormat.RAW, null));
        TDigestState big = new TDigestState(20);
        java.util.Random r = new java.util.Random(3);
        for (int i = 0; i < 500; i++) big.add(r.nextDouble() * 100);
        answer("response.percentiles_big", new InternalTDigestPercentiles("q", new double[] {50, 99.5}, big, false, DocValueFormat.RAW, null));
        answer("response.ranks", new InternalTDigestPercentileRanks("r", new double[] {10, 20}, small, true, DocValueFormat.RAW, null));
        answer("response.percentiles_empty", new InternalTDigestPercentiles("p", new double[] {50}, new TDigestState(100), true, DocValueFormat.RAW, null));
    }
}

package org.opensearch.search.aggregations.metrics;

import org.opensearch.common.io.stream.BytesStreamOutput;
import org.opensearch.common.util.BigArrays;
import org.opensearch.search.aggregations.AggregationBuilders;
import org.opensearch.search.aggregations.AggregatorFactories;
import org.opensearch.search.aggregations.InternalAggregations;

import java.util.HexFormat;
import java.util.List;

/**
 * Writes the bytes OpenSearch 3.10.0 puts on the wire for forwarded {@code cardinality} aggregations
 * (the builders, and the shard's {@code InternalCardinality} for no sketch, a linear-counting one and a
 * HyperLogLog one), one {@code name hex} line each. It is in the metrics package to reach the
 * package-private constructor; compiled as {@code AggRecordings.java} is:
 * {@code java org.opensearch.search.aggregations.metrics.CardRecordings > java-cardinality-3.10.0.txt}.
 */
public final class CardRecordings {
    static void line(String name, BytesStreamOutput out) {
        System.out.println(name + " " + HexFormat.of().formatHex(java.util.Arrays.copyOf(out.bytes().toBytesRef().bytes, out.size())));
    }
    static void answer(String name, AbstractHyperLogLogPlusPlus h) throws Exception {
        BytesStreamOutput out = new BytesStreamOutput();
        InternalAggregations.from(List.of(new InternalCardinality("u", h, null))).writeTo(out);
        line(name, out);
    }
    public static void main(String[] a) throws Exception {
        BytesStreamOutput out = new BytesStreamOutput();
        new AggregatorFactories.Builder()
            .addAggregator(AggregationBuilders.cardinality("u").field("tag"))
            .addAggregator(AggregationBuilders.cardinality("v").field("n").precisionThreshold(100))
            .writeTo(out);
        line("request.cardinality", out);
        HyperLogLogPlusPlus none = new HyperLogLogPlusPlus(14, BigArrays.NON_RECYCLING_INSTANCE, 1);
        answer("response.cardinality_empty", none);
        HyperLogLogPlusPlus few = new HyperLogLogPlusPlus(14, BigArrays.NON_RECYCLING_INSTANCE, 1);
        for (long h : new long[] {0x1234567890abcdefL, 0x0fedcba098765432L, 0x7777777777777777L, 0x0123456789abcdefL}) few.collect(0, h);
        answer("response.cardinality_linear", few);
        HyperLogLogPlusPlus many = new HyperLogLogPlusPlus(4, BigArrays.NON_RECYCLING_INSTANCE, 1);
        java.util.Random r = new java.util.Random(7);
        for (int i = 0; i < 100; i++) many.collect(0, r.nextLong());
        answer("response.cardinality_hll", many);
    }
}

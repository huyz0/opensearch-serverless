package org.opensearch.search.aggregations.bucket.range;

import org.opensearch.common.io.stream.BytesStreamOutput;
import org.opensearch.search.DocValueFormat;
import org.opensearch.search.aggregations.AggregationBuilders;
import org.opensearch.search.aggregations.AggregatorFactories;
import org.opensearch.search.aggregations.InternalAggregation;
import org.opensearch.search.aggregations.InternalAggregations;
import org.opensearch.search.aggregations.metrics.InternalMax;

import java.time.ZoneOffset;
import java.util.HexFormat;
import java.util.List;

/**
 * Writes the bytes OpenSearch 3.10.0 puts on the wire for forwarded {@code range} and {@code date_range}
 * aggregations (the builders, and the shard's {@code InternalRange} and {@code InternalDateRange} with and without
 * a sub-aggregation, keyed or not, with a key asked for and a generated one), one {@code name hex} line each.
 * Compiled as {@code CardRecordings.java} is:
 * {@code java org.opensearch.search.aggregations.bucket.range.RangeRecordings > java-range-3.10.0.txt}.
 */
public final class RangeRecordings {
    static void line(String name, BytesStreamOutput out) {
        System.out.println(name + " " + HexFormat.of().formatHex(java.util.Arrays.copyOf(out.bytes().toBytesRef().bytes, out.size())));
    }
    static void answer(String name, InternalAggregation agg) throws Exception {
        BytesStreamOutput out = new BytesStreamOutput();
        InternalAggregations.from(List.of(agg)).writeTo(out);
        line(name, out);
    }
    static void request(String name, org.opensearch.search.aggregations.AggregationBuilder b) throws Exception {
        BytesStreamOutput out = new BytesStreamOutput();
        new AggregatorFactories.Builder().addAggregator(b).writeTo(out);
        line(name, out);
    }
    public static void main(String[] a) throws Exception {
        request("request.range", AggregationBuilders.range("r").field("n").addUnboundedTo("low", 10).addRange(10, 20).addUnboundedFrom(20));
        request("request.range_keyed", AggregationBuilders.range("r").field("n").keyed(true).addRange("mid", 5, 15));
        request("request.range_subs", AggregationBuilders.range("r").field("n").addRange(0, 10).subAggregation(AggregationBuilders.max("m").field("n")));
        request("request.date_range", AggregationBuilders.dateRange("d").field("t").addUnboundedTo("old", 1000.0).addRange(1000.0, 2000.0));
        request("request.date_range_text", AggregationBuilders.dateRange("d").field("t").keyed(true).addRange("2020", "2020-01-01", "2021-01-01"));

        InternalAggregations subs = InternalAggregations.from(List.of(new InternalMax("m", 4.0, DocValueFormat.RAW, null)));
        DocValueFormat date = new DocValueFormat.DateTime(org.opensearch.index.mapper.DateFieldMapper.DEFAULT_DATE_TIME_FORMATTER, ZoneOffset.UTC, org.opensearch.index.mapper.DateFieldMapper.Resolution.MILLISECONDS);
        double ninf = Double.NEGATIVE_INFINITY, pinf = Double.POSITIVE_INFINITY;
        for (boolean keyed : new boolean[] {false, true}) {
            String k = keyed ? "_keyed" : "";
            answer("response.range" + k, new InternalRange<>("r", List.of(
                new InternalRange.Bucket("low", ninf, 10, 3, InternalAggregations.EMPTY, keyed, DocValueFormat.RAW),
                new InternalRange.Bucket(null, 10, 20, 0, InternalAggregations.EMPTY, keyed, DocValueFormat.RAW),
                new InternalRange.Bucket(null, 20, pinf, 5, InternalAggregations.EMPTY, keyed, DocValueFormat.RAW)), DocValueFormat.RAW, keyed, null));
            answer("response.date_range" + k, new InternalDateRange("d", List.of(
                new InternalDateRange.Bucket("old", ninf, 1000, 3, InternalAggregations.EMPTY, keyed, date),
                new InternalDateRange.Bucket(null, 1000, 2000, 2, InternalAggregations.EMPTY, keyed, date),
                new InternalDateRange.Bucket(null, 2000, pinf, 1, InternalAggregations.EMPTY, keyed, date)), date, keyed, null));
        }
        answer("response.range_subs", new InternalRange<>("r", List.of(
            new InternalRange.Bucket(null, 0, 10, 3, subs, false, DocValueFormat.RAW)), DocValueFormat.RAW, false, null));
        answer("response.date_range_subs", new InternalDateRange("d", List.of(
            new InternalDateRange.Bucket(null, 0, 10, 3, subs, false, date)), date, false, null));
        answer("response.range_empty", new InternalRange<>("r", List.of(), DocValueFormat.RAW, false, null));
    }
}

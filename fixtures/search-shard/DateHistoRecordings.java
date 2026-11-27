package org.opensearch.search.aggregations.bucket.histogram;

import org.opensearch.common.Rounding;
import org.opensearch.common.io.stream.BytesStreamOutput;
import org.opensearch.common.unit.TimeValue;
import org.opensearch.search.DocValueFormat;
import org.opensearch.search.aggregations.AggregationBuilders;
import org.opensearch.search.aggregations.AggregatorFactories;
import org.opensearch.search.aggregations.BucketOrder;
import org.opensearch.search.aggregations.InternalAggregation;
import org.opensearch.search.aggregations.InternalAggregations;
import org.opensearch.search.aggregations.metrics.InternalMax;

import java.time.ZoneOffset;
import java.util.HexFormat;
import java.util.List;

/**
 * Writes the bytes OpenSearch 3.10.0 puts on the wire for forwarded {@code date_histogram} aggregations
 * (the builders, and the shard's {@code InternalDateHistogram} with and without a sub-aggregation, with a
 * calendar and a fixed rounding, with a minimum count of 1, and one empty histogram for each calendar
 * unit), one {@code name hex} line each. Compiled as {@code CardRecordings.java} is:
 * {@code java org.opensearch.search.aggregations.bucket.histogram.DateHistoRecordings > java-date-histogram-3.10.0.txt}.
 */
public final class DateHistoRecordings {
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
            .addAggregator(AggregationBuilders.dateHistogram("d").field("t").calendarInterval(DateHistogramInterval.DAY))
            .addAggregator(AggregationBuilders.dateHistogram("e").field("t").fixedInterval(DateHistogramInterval.seconds(30)).minDocCount(1).offset(5000).order(BucketOrder.key(false)))
            .writeTo(out);
        line("request.date_histogram", out);
        out = new BytesStreamOutput();
        new AggregatorFactories.Builder()
            .addAggregator(AggregationBuilders.dateHistogram("d").field("t").calendarInterval(DateHistogramInterval.DAY)
                .subAggregation(AggregationBuilders.max("m").field("n")))
            .writeTo(out);
        line("request.date_histogram_subs", out);

        InternalAggregations subs = InternalAggregations.from(List.of(new InternalMax("m", 4.0, DocValueFormat.RAW, null)));
        DocValueFormat date = new DocValueFormat.DateTime(org.opensearch.index.mapper.DateFieldMapper.DEFAULT_DATE_TIME_FORMATTER, ZoneOffset.UTC, org.opensearch.index.mapper.DateFieldMapper.Resolution.MILLISECONDS);
        Rounding day = Rounding.builder(Rounding.DateTimeUnit.DAY_OF_MONTH).timeZone(ZoneOffset.UTC).build();
        Rounding sec30 = Rounding.builder(TimeValue.timeValueSeconds(30)).timeZone(ZoneOffset.UTC).build();
        for (boolean withSubs : new boolean[] {false, true}) {
            String sfx = withSubs ? "_subs" : "";
            InternalAggregations in = withSubs ? subs : InternalAggregations.EMPTY;
            InternalDateHistogram.Bucket b1 = new InternalDateHistogram.Bucket(86400000L, 3, false, date, in);
            InternalDateHistogram.Bucket b2 = new InternalDateHistogram.Bucket(172800000L, 1, false, date, InternalAggregations.EMPTY);
            answer("response.date_histogram" + sfx, new InternalDateHistogram("d", List.of(b1, b2), BucketOrder.key(true), 0, 0,
                new InternalDateHistogram.EmptyBucketInfo(day, withSubs ? InternalAggregations.from(List.of(new InternalMax("m", Double.NEGATIVE_INFINITY, DocValueFormat.RAW, null))) : InternalAggregations.EMPTY), date, false, null));
        }
        InternalDateHistogram.Bucket c1 = new InternalDateHistogram.Bucket(30000L, 2, false, date, InternalAggregations.EMPTY);
        answer("response.date_histogram_min1", new InternalDateHistogram("e", List.of(c1), BucketOrder.key(false), 1, 5000, null, date, false, null));
        answer("response.date_histogram_fixed", new InternalDateHistogram("d", List.of(c1), BucketOrder.key(true), 0, 0,
            new InternalDateHistogram.EmptyBucketInfo(sec30, InternalAggregations.EMPTY), date, false, null));
        Rounding.DateTimeUnit[] units = Rounding.DateTimeUnit.values();
        for (Rounding.DateTimeUnit u : units) {
            Rounding r = Rounding.builder(u).timeZone(ZoneOffset.UTC).build();
            answer("response.rounding_" + u.name().toLowerCase(), new InternalDateHistogram("d", List.of(), BucketOrder.key(true), 0, 0,
                new InternalDateHistogram.EmptyBucketInfo(r, InternalAggregations.EMPTY), date, false, null));
        }
    }
}

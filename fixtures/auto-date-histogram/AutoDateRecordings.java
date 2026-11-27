package org.opensearch.search.aggregations.bucket.histogram;

import org.opensearch.common.time.DateFormatter;
import org.opensearch.common.util.BigArrays;
import org.opensearch.common.xcontent.XContentFactory;
import org.opensearch.common.xcontent.json.JsonXContent;
import org.opensearch.core.common.bytes.BytesReference;
import org.opensearch.core.xcontent.DeprecationHandler;
import org.opensearch.core.xcontent.NamedXContentRegistry;
import org.opensearch.core.xcontent.ToXContent;
import org.opensearch.core.xcontent.XContentBuilder;
import org.opensearch.core.xcontent.XContentParser;
import org.opensearch.index.mapper.DateFieldMapper;
import org.opensearch.search.DocValueFormat;
import org.opensearch.search.aggregations.InternalAggregation;
import org.opensearch.search.aggregations.InternalAggregations;

import java.time.ZoneOffset;
import java.util.ArrayList;
import java.util.List;
import java.util.Map;

/**
 * Prints what OpenSearch 3.10.0 does with an {@code auto_date_histogram}: how bodies parse (and fail),
 * and how shard results reduce, one {@code name text} line each:
 * {@code java org.opensearch.search.aggregations.bucket.histogram.AutoDateRecordings > java-auto-date-3.10.0.txt}.
 */
public final class AutoDateRecordings {
    static final NamedXContentRegistry REGISTRY = new NamedXContentRegistry(new org.opensearch.search.SearchModule(org.opensearch.common.settings.Settings.EMPTY, List.of()).getNamedXContents());
    static final DocValueFormat FORMAT = new DocValueFormat.DateTime(DateFormatter.forPattern("strict_date_optional_time"), ZoneOffset.UTC, DateFieldMapper.Resolution.MILLISECONDS);
    static final long DAY = 86_400_000L;
    static final long HOUR = 3_600_000L;
    static final long T0 = 1_700_000_000_000L - (1_700_000_000_000L % DAY); // a UTC midnight

    static String chain(Throwable e) {
        Throwable root = e;
        String chain = e.getClass().getName() + ": " + e.getMessage();
        while (root.getCause() != null) { root = root.getCause(); chain += " <- " + root.getClass().getName() + ": " + root.getMessage(); }
        return chain.replace('\n', ' ');
    }

    static void parse(String body) {
        try (XContentParser parser = JsonXContent.jsonXContent.createParser(
            REGISTRY, DeprecationHandler.THROW_UNSUPPORTED_OPERATION, body)) {
            parser.nextToken();
            AutoDateHistogramAggregationBuilder a = AutoDateHistogramAggregationBuilder.PARSER.parse(parser, "p");
            System.out.println("parse " + body + " -> ok field=" + a.field() + " buckets=" + a.getNumBuckets()
                + " min=" + a.getMinimumIntervalExpression() + " format=" + a.format() + " missing=" + a.missing());
        } catch (Exception e) {
            System.out.println("parse " + body + " -> " + chain(e));
        }
    }

    /** A shard's result: the rounding index, the inner interval and (key offset in {@code step} millis, count) pairs. */
    static InternalAutoDateHistogram shard(String minimum, int target, int roundingIdx, long inner, long step, long... offsetCount) {
        AutoDateHistogramAggregationBuilder.RoundingInfo[] infos = AutoDateHistogramAggregationBuilder.buildRoundings(ZoneOffset.UTC, minimum);
        List<InternalAutoDateHistogram.Bucket> buckets = new ArrayList<>();
        for (int i = 0; i < offsetCount.length; i += 2) {
            buckets.add(new InternalAutoDateHistogram.Bucket(T0 + offsetCount[i] * step, offsetCount[i + 1], FORMAT, InternalAggregations.EMPTY));
        }
        InternalAutoDateHistogram.BucketInfo info = new InternalAutoDateHistogram.BucketInfo(infos, roundingIdx, InternalAggregations.EMPTY);
        return new InternalAutoDateHistogram("p", buckets, target, info, FORMAT, null, inner);
    }

    static void reduce(String name, boolean typed, InternalAutoDateHistogram... shards) throws Exception {
        InternalAggregation.ReduceContext context = InternalAggregation.ReduceContext.forFinalReduction(
            BigArrays.NON_RECYCLING_INSTANCE, null, i -> {}, org.opensearch.search.aggregations.pipeline.PipelineAggregator.PipelineTree.EMPTY);
        InternalAggregation r = shards[0].reduce(new ArrayList<InternalAggregation>(List.of(shards)), context);
        XContentBuilder x = XContentFactory.jsonBuilder();
        x.startObject();
        r.toXContent(x, typed ? new ToXContent.MapParams(Map.of("typed_keys", "true")) : ToXContent.EMPTY_PARAMS);
        x.endObject();
        System.out.println(name + " " + BytesReference.bytes(x).utf8ToString());
    }

    public static void main(String[] args) throws Exception {
        for (String body : new String[] {
            "{\"field\":\"t\"}", "{\"field\":\"t\",\"buckets\":3}", "{\"field\":\"t\",\"buckets\":1}", "{\"field\":\"t\",\"buckets\":0}",
            "{\"field\":\"t\",\"buckets\":-1}", "{\"field\":\"t\",\"buckets\":10000}", "{\"field\":\"t\",\"buckets\":\"5\"}", "{\"field\":\"t\",\"buckets\":2.5}",
            "{\"field\":\"t\",\"minimum_interval\":\"hour\"}", "{\"field\":\"t\",\"minimum_interval\":\"minute\"}", "{\"field\":\"t\",\"minimum_interval\":\"second\"}",
            "{\"field\":\"t\",\"minimum_interval\":\"day\"}", "{\"field\":\"t\",\"minimum_interval\":\"month\"}", "{\"field\":\"t\",\"minimum_interval\":\"year\"}",
            "{\"field\":\"t\",\"minimum_interval\":\"week\"}", "{\"field\":\"t\",\"minimum_interval\":\"1h\"}", "{\"field\":\"t\",\"minimum_interval\":\"\"}", "{\"field\":\"t\",\"minimum_interval\":1}",
            "{\"field\":\"t\",\"format\":\"yyyy\"}", "{\"field\":\"t\",\"missing\":\"2020-01-01\"}", "{\"field\":\"t\",\"time_zone\":\"UTC\"}", "{\"field\":\"t\",\"time_zone\":\"+01:00\"}",
            "{\"field\":\"t\",\"script\":{\"source\":\"1\"}}", "{\"field\":\"t\",\"interval\":\"day\"}", "{\"field\":\"t\",\"foo\":1}", "{}", "{\"buckets\":3}",
            "{\"field\":1}", "{\"field\":null}", "{\"field\":\"t\",\"buckets\":null}", "{\"field\":\"t\",\"buckets\":[3]}", "{\"field\":\"t\",\"keyed\":true}", "{\"field\":\"t\",\"offset\":\"1h\"}"
        }) { parse(body); }
        // inner interval 1: 5 daily buckets with a gap, one shard
        reduce("reduce.days5", false, shard(null, 10, 3, 1, DAY, 0, 2, 1, 3, 2, 1, 4, 5, 5, 1));
        // 30 days over target 10 at idx 3 -> coarser
        long[] thirty = new long[60];
        for (int i = 0; i < 30; i++) { thirty[2 * i] = i; thirty[2 * i + 1] = i + 1; }
        reduce("reduce.days30.t10", false, shard(null, 10, 3, 1, DAY, thirty));
        reduce("reduce.days30.t10.typed", true, shard(null, 10, 3, 1, DAY, thirty));
        reduce("reduce.days30.t3", false, shard(null, 3, 3, 1, DAY, thirty));
        reduce("reduce.days30.t100", false, shard(null, 100, 3, 1, DAY, thirty));
        reduce("reduce.days30.t30", false, shard(null, 30, 3, 1, DAY, thirty));
        reduce("reduce.days30.t29", false, shard(null, 29, 3, 1, DAY, thirty));
        // two shards at different roundings: hours (idx 2) and days (idx 3)
        reduce("reduce.mixed", false, shard(null, 10, 2, 1, HOUR, 0, 1, 1, 2, 5, 3), shard(null, 10, 3, 1, DAY, 0, 4, 2, 6));
        // two shards at the same rounding with overlapping buckets
        reduce("reduce.overlap", false, shard(null, 10, 3, 1, DAY, 0, 1, 1, 2), shard(null, 10, 3, 1, DAY, 1, 3, 3, 4));
        reduce("reduce.empty", false, shard(null, 10, 3, 1, DAY));
        reduce("reduce.hours48", false, shard(null, 10, 2, 1, HOUR, java.util.stream.LongStream.range(0, 96).map(i -> i % 2 == 0 ? i / 2 : 1).toArray()));
        reduce("reduce.minimum.hour", false, shard("hour", 10, 0, 1, HOUR, 0, 1, 3, 2));
        reduce("reduce.inner3", false, shard(null, 10, 2, 3, HOUR, 0, 1, 1, 2, 2, 3));
    }
}

package org.opensearch.search.aggregations.pipeline;

import org.opensearch.common.util.BigArrays;
import org.opensearch.common.xcontent.XContentFactory;
import org.opensearch.core.common.bytes.BytesReference;
import org.opensearch.core.xcontent.ToXContent;
import org.opensearch.core.xcontent.XContentBuilder;
import org.opensearch.search.DocValueFormat;
import org.opensearch.search.aggregations.BucketOrder;
import org.opensearch.search.aggregations.InternalAggregation;
import org.opensearch.search.aggregations.InternalAggregations;
import org.opensearch.search.aggregations.bucket.histogram.InternalHistogram;
import org.opensearch.search.aggregations.metrics.InternalAvg;
import org.opensearch.search.aggregations.metrics.InternalSum;
import org.opensearch.search.sort.FieldSortBuilder;
import org.opensearch.search.sort.SortOrder;

import java.util.ArrayList;
import java.util.List;

/**
 * Prints the body OpenSearch 3.10.0 writes for each parent pipeline aggregation ({@code derivative},
 * {@code cumulative_sum}, {@code bucket_sort}) reduced over a {@code histogram} of four buckets, one
 * {@code name json} line each. Compiled in the pipeline package for its package-private constructors:
 * {@code java org.opensearch.search.aggregations.pipeline.PipelineParent > java-parent-3.10.0.txt}.
 */
public final class PipelineParent {
    static InternalHistogram histogram(boolean gap) {
        return histogram(gap, false);
    }

    /** With {@code empty}, the third bucket holds no document (a bucket a reduce filled in). */
    static InternalHistogram histogram(boolean gap, boolean empty) {
        // key, doc count, sum, avg sum, avg count; the avg of the third bucket has no value when gap is set.
        double[][] rows = {{0, 3, 10, 6, 2}, {10, 5, 25, 9, 3}, {20, empty ? 0 : 2, empty ? 0 : 7, 0, gap || empty ? 0 : 1}, {30, 4, 12, 8, 2}};
        List<InternalHistogram.Bucket> buckets = new ArrayList<>();
        for (double[] r : rows) {
            InternalAggregations subs = InternalAggregations.from(List.of(
                new InternalSum("s", r[2], DocValueFormat.RAW, null),
                new InternalAvg("a", r[3], (long) r[4], DocValueFormat.RAW, null)));
            buckets.add(new InternalHistogram.Bucket(r[0], (long) r[1], false, DocValueFormat.RAW, subs));
        }
        return new InternalHistogram("h", buckets, BucketOrder.key(true), 1, null, DocValueFormat.RAW, false, null);
    }

    static void run(String name, AbstractPipelineAggregationBuilder<?> builder, InternalHistogram input) throws Exception {
        PipelineAggregator aggregator = builder.create();
        InternalAggregation.ReduceContext context = InternalAggregation.ReduceContext.forFinalReduction(
            BigArrays.NON_RECYCLING_INSTANCE, null, i -> {}, PipelineAggregator.PipelineTree.EMPTY);
        InternalAggregation result = aggregator.reduce(input, context);
        XContentBuilder x = XContentFactory.jsonBuilder();
        x.startObject();
        result.toXContent(x, ToXContent.EMPTY_PARAMS);
        x.endObject();
        System.out.println(name + " " + BytesReference.bytes(x).utf8ToString());
    }

    static void fail(String name, java.util.function.Supplier<AbstractPipelineAggregationBuilder<?>> builder, InternalHistogram input) {
        try {
            run(name, builder.get(), input);
        } catch (Exception e) {
            System.out.println(name + " " + e.getClass().getName() + " " + e.getMessage());
        }
    }

    static void sibling(String name, BucketMetricsPipelineAggregationBuilder<?> builder, InternalHistogram input) throws Exception {
        PipelineAggregator aggregator = builder.create();
        InternalAggregation.ReduceContext context = InternalAggregation.ReduceContext.forFinalReduction(
            BigArrays.NON_RECYCLING_INSTANCE, null, i -> {}, PipelineAggregator.PipelineTree.EMPTY);
        InternalAggregation result = ((SiblingPipelineAggregator) aggregator).doReduce(InternalAggregations.from(List.of(input)), context);
        XContentBuilder x = XContentFactory.jsonBuilder();
        x.startObject();
        result.toXContent(x, ToXContent.EMPTY_PARAMS);
        x.endObject();
        System.out.println(name + " " + BytesReference.bytes(x).utf8ToString());
    }

    static FieldSortBuilder sort(String field, SortOrder order) {
        return new FieldSortBuilder(field).order(order);
    }

    public static void main(String[] args) throws Exception {
        for (boolean gap : new boolean[] {false, true}) {
            String g = gap ? "gap." : "";
            for (String path : new String[] {"s", "_count", "a"}) {
                for (BucketHelpers.GapPolicy policy : BucketHelpers.GapPolicy.values()) {
                    String p = policy.name().toLowerCase(java.util.Locale.ROOT);
                    fail(g + "derivative." + path + "." + p, () -> new DerivativePipelineAggregationBuilder("d", path).gapPolicy(policy), histogram(gap));
                }
                fail(g + "cumulative_sum." + path, () -> new CumulativeSumPipelineAggregationBuilder("c", path), histogram(gap));
            }
        }
        fail("derivative.unit", () -> new DerivativePipelineAggregationBuilder("d", "s").unit("1d"), histogram(false));
        fail("sort.s_desc.from1.size2", () -> new BucketSortPipelineAggregationBuilder("b", List.of(sort("s", SortOrder.DESC))).from(1).size(2), histogram(false));
        fail("sort.count_asc", () -> new BucketSortPipelineAggregationBuilder("b", List.of(sort("_count", SortOrder.ASC))), histogram(false));
        fail("sort.key_desc", () -> new BucketSortPipelineAggregationBuilder("b", List.of(sort("_key", SortOrder.DESC))), histogram(false));
        fail("sort.none.from1.size2", () -> new BucketSortPipelineAggregationBuilder("b", List.of()).from(1).size(2), histogram(false));
        fail("sort.none.size0", () -> new BucketSortPipelineAggregationBuilder("b", List.of()).size(0), histogram(false));
        fail("sort.none.from9", () -> new BucketSortPipelineAggregationBuilder("b", List.of()).from(9), histogram(false));
        fail("gap.sort.a_desc.skip", () -> new BucketSortPipelineAggregationBuilder("b", List.of(sort("a", SortOrder.DESC))), histogram(true));
        fail("gap.sort.a_desc.zeros", () -> new BucketSortPipelineAggregationBuilder("b", List.of(sort("a", SortOrder.DESC))).gapPolicy(BucketHelpers.GapPolicy.INSERT_ZEROS), histogram(true));
        fail("gap.sort.a_asc.skip", () -> new BucketSortPipelineAggregationBuilder("b", List.of(sort("a", SortOrder.ASC))), histogram(true));
        fail("sort.two", () -> new BucketSortPipelineAggregationBuilder("b", List.of(sort("_count", SortOrder.ASC), sort("s", SortOrder.DESC))), histogram(false));
        // A bucket with no document is a gap for a path other than _count.
        for (BucketHelpers.GapPolicy policy : BucketHelpers.GapPolicy.values()) {
            String p = policy.name().toLowerCase(java.util.Locale.ROOT);
            fail("empty.derivative.s." + p, () -> new DerivativePipelineAggregationBuilder("d", "s").gapPolicy(policy), histogram(false, true));
            fail("empty.derivative._count." + p, () -> new DerivativePipelineAggregationBuilder("d", "_count").gapPolicy(policy), histogram(false, true));
            fail("empty.sort.s_desc." + p, () -> new BucketSortPipelineAggregationBuilder("b", List.of(sort("s", SortOrder.DESC))).gapPolicy(policy), histogram(false, true));
            fail("empty.sort._count_desc." + p, () -> new BucketSortPipelineAggregationBuilder("b", List.of(sort("_count", SortOrder.DESC))).gapPolicy(policy), histogram(false, true));
            sibling("empty.sum_bucket.s." + p, (BucketMetricsPipelineAggregationBuilder<?>) new SumBucketPipelineAggregationBuilder("p", "h>s").gapPolicy(policy), histogram(false, true));
            sibling("empty.avg_bucket._count." + p, (BucketMetricsPipelineAggregationBuilder<?>) new AvgBucketPipelineAggregationBuilder("p", "h>_count").gapPolicy(policy), histogram(false, true));
        }
        fail("empty.cumulative_sum.s", () -> new CumulativeSumPipelineAggregationBuilder("c", "s"), histogram(false, true));
        fail("gap.sort.two.skip", () -> new BucketSortPipelineAggregationBuilder("b", List.of(sort("a", SortOrder.DESC), sort("_count", SortOrder.ASC))), histogram(true));
        fail("gap.sort.two.zeros", () -> new BucketSortPipelineAggregationBuilder("b", List.of(sort("_count", SortOrder.ASC), sort("a", SortOrder.DESC))).gapPolicy(BucketHelpers.GapPolicy.INSERT_ZEROS), histogram(true));
        fail("sort.missing_path", () -> new BucketSortPipelineAggregationBuilder("b", List.of(sort("zz", SortOrder.DESC))), histogram(false));
        fail("derivative.missing_path", () -> new DerivativePipelineAggregationBuilder("d", "zz"), histogram(false));
        fail("cumulative_sum.missing_path", () -> new CumulativeSumPipelineAggregationBuilder("c", "zz"), histogram(false));
    }
}
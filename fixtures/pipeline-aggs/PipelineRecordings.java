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
import org.opensearch.search.aggregations.bucket.terms.LongTerms;
import org.opensearch.search.aggregations.bucket.terms.TermsAggregator;
import org.opensearch.search.aggregations.metrics.InternalAvg;

import java.util.ArrayList;
import java.util.List;

/**
 * Prints the body OpenSearch 3.10.0 writes for each sibling pipeline aggregation ({@code avg_bucket},
 * {@code sum_bucket}, {@code min_bucket}, {@code max_bucket}, {@code stats_bucket}) over a {@code terms} aggregation
 * of four buckets, one {@code name json} line each. Compiled in the pipeline package for its package-private
 * constructors: {@code java org.opensearch.search.aggregations.pipeline.PipelineRecordings > java-pipeline-3.10.0.txt}.
 */
public final class PipelineRecordings {
    static LongTerms terms(double[][] rows) {
        List<LongTerms.Bucket> buckets = new ArrayList<>();
        for (double[] r : rows) {
            InternalAggregations subs = InternalAggregations.from(List.of(
                new InternalAvg("a", r[2], (long) r[3], DocValueFormat.RAW, null)));
            buckets.add(new LongTerms.Bucket((long) r[0], (long) r[1], subs, false, 0, DocValueFormat.RAW));
        }
        return new LongTerms("t", BucketOrder.count(false), BucketOrder.count(false), null, DocValueFormat.RAW, 10, false, 0, buckets, 0,
            new TermsAggregator.BucketCountThresholds(1, 0, 10, 10));
    }

    static void run(String name, BucketMetricsPipelineAggregationBuilder<?> builder, LongTerms input) throws Exception {
        PipelineAggregator aggregator = builder.create();
        InternalAggregation.ReduceContext context = InternalAggregation.ReduceContext.forFinalReduction(
            BigArrays.NON_RECYCLING_INSTANCE, null, i -> {}, PipelineAggregator.PipelineTree.EMPTY);
        InternalAggregation result = ((SiblingPipelineAggregator) aggregator).doReduce(InternalAggregations.from(List.of(input)), context);
        XContentBuilder x = XContentFactory.jsonBuilder();
        x.startObject();
        result.toXContent(x, ToXContent.EMPTY_PARAMS);
        x.endObject();
        System.out.println(name + " " + result.getType() + " " + BytesReference.bytes(x).utf8ToString());
    }

    static void fail(String name, BucketMetricsPipelineAggregationBuilder<?> builder, LongTerms input) {
        try {
            run(name, builder, input);
        } catch (Exception e) {
            System.out.println(name + " " + e.getClass().getName() + " " + e.getMessage());
        }
    }

    public static void main(String[] args) throws Exception {
        // key, doc count, avg sum, avg count: the second bucket's average has no value (NaN).
        double[][] rows = {{1, 5, 10, 1}, {2, 3, 0, 0}, {3, 4, 4, 1}, {4, 1, -2, 1}};
        double[][] ties = {{1, 2, 3, 1}, {2, 2, 3, 1}, {3, 7, 3, 1}};
        for (String path : new String[] {"t>a", "t>_count"}) {
            for (BucketHelpers.GapPolicy gap : BucketHelpers.GapPolicy.values()) {
                String tag = (path.endsWith("a") ? "a" : "count") + "." + gap.getName();
                run("avg." + tag, new AvgBucketPipelineAggregationBuilder("p", path).gapPolicy(gap), terms(rows));
                run("sum." + tag, new SumBucketPipelineAggregationBuilder("p", path).gapPolicy(gap), terms(rows));
                run("min." + tag, new MinBucketPipelineAggregationBuilder("p", path).gapPolicy(gap), terms(rows));
                run("max." + tag, new MaxBucketPipelineAggregationBuilder("p", path).gapPolicy(gap), terms(rows));
                run("stats." + tag, new StatsBucketPipelineAggregationBuilder("p", path).gapPolicy(gap), terms(rows));
            }
        }
        run("min.ties", new MinBucketPipelineAggregationBuilder("p", "t>a"), terms(ties));
        run("max.ties", new MaxBucketPipelineAggregationBuilder("p", "t>a"), terms(ties));
        run("avg.empty", new AvgBucketPipelineAggregationBuilder("p", "t>_count"), terms(new double[0][]));
        run("sum.empty", new SumBucketPipelineAggregationBuilder("p", "t>_count"), terms(new double[0][]));
        run("min.empty", new MinBucketPipelineAggregationBuilder("p", "t>_count"), terms(new double[0][]));
        run("max.empty", new MaxBucketPipelineAggregationBuilder("p", "t>_count"), terms(new double[0][]));
        run("stats.empty", new StatsBucketPipelineAggregationBuilder("p", "t>_count"), terms(new double[0][]));
        double[][] one = {{1, 5, 10, 1}};
        fail("fail.unknown_agg", new AvgBucketPipelineAggregationBuilder("p", "x>a"), terms(one));
        fail("fail.unknown_sub", new AvgBucketPipelineAggregationBuilder("p", "t>zz"), terms(one));
        fail("fail.not_multibucket", new AvgBucketPipelineAggregationBuilder("p", "p"), terms(one));
        fail("fail.deep_value", new AvgBucketPipelineAggregationBuilder("p", "t>a.max"), terms(one));
        run("avg.format", new AvgBucketPipelineAggregationBuilder("p", "t>_count").format("0.00"), terms(rows));
    }
}

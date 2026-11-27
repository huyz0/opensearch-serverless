package org.opensearch.search.aggregations.pipeline;

import org.opensearch.common.util.BigArrays;
import org.opensearch.common.xcontent.XContentFactory;
import org.opensearch.core.common.bytes.BytesReference;
import org.opensearch.core.xcontent.ToXContent;
import org.opensearch.core.xcontent.XContentBuilder;
import org.opensearch.search.DocValueFormat;
import org.opensearch.search.aggregations.AggregatorFactories;
import org.opensearch.search.aggregations.BucketOrder;
import org.opensearch.search.aggregations.InternalAggregation;
import org.opensearch.search.aggregations.InternalAggregations;
import org.opensearch.search.aggregations.bucket.histogram.HistogramAggregationBuilder;
import org.opensearch.search.aggregations.bucket.histogram.InternalHistogram;
import org.opensearch.search.aggregations.metrics.AvgAggregationBuilder;
import org.opensearch.search.aggregations.metrics.InternalAvg;
import org.opensearch.search.aggregations.metrics.InternalSum;
import org.opensearch.search.aggregations.metrics.SumAggregationBuilder;

import java.util.ArrayList;
import java.util.List;

/**
 * Prints the response body of a final reduce ({@code InternalAggregations.topLevelReduce}) of one or two shards'
 * results, with a derivative inside a {@code histogram} and a sibling pipeline beside it, to show the order the
 * aggregations and the pipeline results are listed in, at OpenSearch 3.10.0. Compiled as {@code PipelineRecordings.java} is.
 */
public final class PipelineOrder {
    static InternalAggregations shard(double scale) {
        List<InternalHistogram.Bucket> buckets = new ArrayList<>();
        for (int i = 0; i < 3; i++) {
            InternalAggregations subs = InternalAggregations.from(List.of(
                new InternalSum("s", scale * (i + 1), DocValueFormat.RAW, null),
                new InternalAvg("zz", scale * 2, 1, DocValueFormat.RAW, null)));
            buckets.add(new InternalHistogram.Bucket(i * 10, 2, false, DocValueFormat.RAW, subs));
        }
        return InternalAggregations.from(List.of(
            new InternalHistogram("h", buckets, BucketOrder.key(true), 1, null, DocValueFormat.RAW, false, null),
            new InternalSum("zz", scale, DocValueFormat.RAW, null),
            new InternalSum("m", scale, DocValueFormat.RAW, null)));
    }

    public static void main(String[] args) throws Exception {
        AggregatorFactories.Builder builder = AggregatorFactories.builder()
            .addAggregator(new HistogramAggregationBuilder("h").field("n").interval(10)
                .subAggregation(new SumAggregationBuilder("s").field("n"))
                .subAggregation(new AvgAggregationBuilder("zz").field("n"))
                .subAggregation(new DerivativePipelineAggregationBuilder("d", "s"))
                .subAggregation(new CumulativeSumPipelineAggregationBuilder("c", "s")))
            .addAggregator(new SumAggregationBuilder("zz").field("n"))
            .addAggregator(new SumAggregationBuilder("m").field("n"))
            .addPipelineAggregator(new SumBucketPipelineAggregationBuilder("p", "h>s"))
            .addPipelineAggregator(new AvgBucketPipelineAggregationBuilder("q", "h>s"));
        for (int shards = 1; shards <= 2; shards++) {
            List<InternalAggregations> results = new ArrayList<>();
            for (int i = 0; i < shards; i++) { results.add(shard(i + 1)); }
            InternalAggregation.ReduceContext context = InternalAggregation.ReduceContext.forFinalReduction(
                BigArrays.NON_RECYCLING_INSTANCE, null, i -> {}, builder.buildPipelineTree());
            InternalAggregations reduced = InternalAggregations.topLevelReduce(results, context);
            XContentBuilder x = XContentFactory.jsonBuilder();
            x.startObject();
            reduced.toXContent(x, ToXContent.EMPTY_PARAMS);
            x.endObject();
            System.out.println("shards" + shards + " " + BytesReference.bytes(x).utf8ToString());
        }
    }
}

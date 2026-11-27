import org.opensearch.common.io.stream.BytesStreamOutput;
import org.opensearch.search.aggregations.AggregationBuilders;
import org.opensearch.search.aggregations.AggregatorFactories;
import org.opensearch.search.aggregations.PipelineAggregatorBuilders;
import org.opensearch.search.aggregations.BucketOrder;
import org.opensearch.search.aggregations.pipeline.BucketHelpers.GapPolicy;
import org.opensearch.search.aggregations.pipeline.BucketSortPipelineAggregationBuilder;
import org.opensearch.search.aggregations.PipelineAggregationBuilder;
import org.opensearch.search.sort.FieldSortBuilder;
import org.opensearch.search.sort.SortOrder;

import java.util.HexFormat;
import java.util.List;

/**
 * Writes the bytes OpenSearch 3.10.0's own classes put on the wire for the pipeline aggregation
 * builders of a forwarded search ({@code AggregatorFactories.Builder.writeTo} with pipeline
 * aggregations), one {@code name hex} line each: {@code java PipelineWireRecordings >
 * java-pipeline-wire-3.10.0.txt}, compiled as {@code AggRecordings} is.
 */
public final class PipelineWireRecordings {
    static void line(String name, AggregatorFactories.Builder b) throws Exception {
        BytesStreamOutput out = new BytesStreamOutput();
        b.writeTo(out);
        System.out.println(name + " " + HexFormat.of().formatHex(java.util.Arrays.copyOf(out.bytes().toBytesRef().bytes, out.size())));
    }
    static AggregatorFactories.Builder top(PipelineAggregationBuilder p) {
        return new AggregatorFactories.Builder()
            .addAggregator(AggregationBuilders.terms("t").field("tag").subAggregation(AggregationBuilders.sum("s").field("n")))
            .addPipelineAggregator(p);
    }
    static AggregatorFactories.Builder inside(PipelineAggregationBuilder p) {
        return new AggregatorFactories.Builder().addAggregator(AggregationBuilders.dateHistogram("h").field("ts").calendarInterval(new org.opensearch.search.aggregations.bucket.histogram.DateHistogramInterval("1d"))
            .subAggregation(AggregationBuilders.sum("s").field("n"))
            .subAggregation(p));
    }
    public static void main(String[] a) throws Exception {
        line("avg_bucket", top(PipelineAggregatorBuilders.avgBucket("p", "t>s")));
        line("sum_bucket", top(PipelineAggregatorBuilders.sumBucket("p", "t>s")));
        line("min_bucket", top(PipelineAggregatorBuilders.minBucket("p", "t>s")));
        line("max_bucket", top(PipelineAggregatorBuilders.maxBucket("p", "t>s")));
        line("stats_bucket", top(PipelineAggregatorBuilders.statsBucket("p", "t>s")));
        line("avg_bucket.options", top(PipelineAggregatorBuilders.avgBucket("p", "t>s").gapPolicy(GapPolicy.INSERT_ZEROS).format("0.0")));
        line("derivative", inside(PipelineAggregatorBuilders.derivative("d", "s")));
        line("derivative.options", inside(PipelineAggregatorBuilders.derivative("d", "s").gapPolicy(GapPolicy.INSERT_ZEROS).format("#.#").unit("1d")));
        line("cumulative_sum", inside(PipelineAggregatorBuilders.cumulativeSum("c", "s")));
        line("cumulative_sum.format", inside(PipelineAggregatorBuilders.cumulativeSum("c", "s").format("0.00")));
        line("bucket_sort", inside(new BucketSortPipelineAggregationBuilder("b", List.of(new FieldSortBuilder("s").order(SortOrder.DESC), new FieldSortBuilder("_key"))).from(1).size(2)));
        line("bucket_sort.window", inside(new BucketSortPipelineAggregationBuilder("b", null).size(3).gapPolicy(GapPolicy.INSERT_ZEROS)));
        line("two", new AggregatorFactories.Builder().addAggregator(AggregationBuilders.terms("t").field("tag").subAggregation(AggregationBuilders.sum("s").field("n")))
            .addPipelineAggregator(PipelineAggregatorBuilders.maxBucket("m", "t>s")).addPipelineAggregator(PipelineAggregatorBuilders.avgBucket("a", "t>s")));
    }
}

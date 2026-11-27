package org.opensearch.search.aggregations.pipeline;

import org.opensearch.search.aggregations.bucket.histogram.InternalHistogram;

import java.util.List;

/**
 * Prints the body OpenSearch 3.10.0 writes for {@code serial_diff} reduced over the four-bucket
 * {@code histogram} of {@link PipelineParent}, one {@code name json} line each, and the builder
 * refusals: {@code java org.opensearch.search.aggregations.pipeline.PipelineSerial > java-serial-3.10.0.txt}.
 */
public final class PipelineSerial {
    public static void main(String[] args) throws Exception {
        for (boolean gap : new boolean[] {false, true}) {
            String g = gap ? "gap." : "";
            for (String path : new String[] {"s", "_count", "a"}) {
                for (int lag : new int[] {1, 2, 3, 5}) {
                    for (BucketHelpers.GapPolicy policy : BucketHelpers.GapPolicy.values()) {
                        String p = policy.name().toLowerCase(java.util.Locale.ROOT);
                        PipelineParent.fail(g + "serial." + path + ".lag" + lag + "." + p,
                            () -> new SerialDiffPipelineAggregationBuilder("d", path).lag(lag).gapPolicy(policy), PipelineParent.histogram(gap));
                    }
                }
            }
        }
        for (BucketHelpers.GapPolicy policy : BucketHelpers.GapPolicy.values()) {
            String p = policy.name().toLowerCase(java.util.Locale.ROOT);
            PipelineParent.fail("empty.serial.s.lag1." + p, () -> new SerialDiffPipelineAggregationBuilder("d", "s").gapPolicy(policy), PipelineParent.histogram(false, true));
            PipelineParent.fail("empty.serial._count.lag2." + p, () -> new SerialDiffPipelineAggregationBuilder("d", "_count").lag(2).gapPolicy(policy), PipelineParent.histogram(false, true));
        }
        PipelineParent.fail("serial.lag0", () -> new SerialDiffPipelineAggregationBuilder("d", "s").lag(0), PipelineParent.histogram(false));
        PipelineParent.fail("serial.lag_neg", () -> new SerialDiffPipelineAggregationBuilder("d", "s").lag(-1), PipelineParent.histogram(false));
        PipelineParent.fail("serial.missing_path", () -> new SerialDiffPipelineAggregationBuilder("d", "zz"), PipelineParent.histogram(false));
        PipelineParent.fail("serial.format", () -> new SerialDiffPipelineAggregationBuilder("d", "s").format("0.0"), PipelineParent.histogram(false));
    }
}

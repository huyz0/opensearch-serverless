package org.opensearch.search.aggregations.pipeline;

import org.opensearch.action.ActionRequestValidationException;
import org.opensearch.search.aggregations.AggregatorFactories;
import org.opensearch.search.aggregations.PipelineAggregationBuilder;
import org.opensearch.search.aggregations.bucket.terms.TermsAggregationBuilder;
import org.opensearch.search.aggregations.metrics.AvgAggregationBuilder;

/**
 * Prints what request validation says (the "Validation Failed" messages, or {@code ok}) of one sibling pipeline
 * aggregation beside a {@code terms} "t" holding an {@code avg} "a", and an {@code avg} "m" at the top level, for
 * several {@code buckets_path}s, at OpenSearch 3.10.0. Compiled as {@code PipelineRecordings.java} is.
 */
public final class PipelineValidation {
    static void check(String path) {
        AggregatorFactories.Builder root = AggregatorFactories.builder()
            .addAggregator(new TermsAggregationBuilder("t").field("f").subAggregation(new AvgAggregationBuilder("a").field("n")))
            .addAggregator(new AvgAggregationBuilder("m").field("n"))
            .addPipelineAggregator(new AvgBucketPipelineAggregationBuilder("p", path));
        ActionRequestValidationException e = root.validate(null);
        System.out.println(path + " " + (e == null ? "ok" : String.join(" | ", e.validationErrors())));
    }

    public static void main(String[] args) {
        for (String path : new String[] {"t>a", "t>_count", "x>a", "t>zz", "p", "m", "t", "t>a.max", "t>a.value", "t>t>a", "", "t>"}) {
            try {
                check(path);
            } catch (Exception e) {
                System.out.println(path + " " + e.getClass().getName() + " " + e.getMessage());
            }
        }
    }
}

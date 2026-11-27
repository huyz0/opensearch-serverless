package org.opensearch.search.aggregations.pipeline;

import org.opensearch.common.xcontent.json.JsonXContent;
import org.opensearch.core.xcontent.DeprecationHandler;
import org.opensearch.core.xcontent.NamedXContentRegistry;
import org.opensearch.core.xcontent.XContentParser;

/**
 * Prints what {@code avg_bucket}'s body parser makes of several bodies (the builder's path, gap policy and format, or
 * the exception's class and message), at OpenSearch 3.10.0. Compiled as {@code PipelineRecordings.java} is.
 */
public final class PipelineParse {
    public static void main(String[] args) throws Exception {
        String[] bodies = {
            "{}", "{\"buckets_path\":1}", "{\"buckets_path\":\"t>a\",\"gap_policy\":\"nope\"}", "{\"buckets_path\":\"t>a\",\"foo\":1}",
            "{\"buckets_path\":[\"t>a\"]}", "{\"buckets_path\":\"t>a\",\"gap_policy\":\"insert_zeros\"}", "{\"buckets_path\":\"t>a\",\"gap_policy\":\"SKIP\"}",
            "{\"buckets_path\":\"t>a\",\"format\":\"0.0\"}", "{\"buckets_path\":\"t>a\",\"gap_policy\":1}", "{\"buckets_path\":\"t>a\",\"gap_policy\":\"skip\",\"gap_policy\":\"skip\"}"
        };
        for (String body : bodies) {
            try (XContentParser parser = JsonXContent.jsonXContent.createParser(
                NamedXContentRegistry.EMPTY, DeprecationHandler.THROW_UNSUPPORTED_OPERATION, body)) {
                parser.nextToken(); // the body's start
                AvgBucketPipelineAggregationBuilder b = (AvgBucketPipelineAggregationBuilder) AvgBucketPipelineAggregationBuilder.PARSER.parse("p", parser);
                System.out.println(body + " -> ok gap=" + b.gapPolicy().getName() + " format=" + b.format() + " paths=" + String.join(",", b.getBucketsPaths()));
            } catch (Exception e) {
                Throwable root = e;
                String chain = e.getClass().getName() + ": " + e.getMessage();
                while (root.getCause() != null) { root = root.getCause(); chain += " <- " + root.getClass().getName() + ": " + root.getMessage(); }
                System.out.println(body + " -> " + chain);
            }
        }
    }
}

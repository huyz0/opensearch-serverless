package org.opensearch.search.aggregations.pipeline;

import org.opensearch.action.ActionRequestValidationException;
import org.opensearch.common.xcontent.json.JsonXContent;
import org.opensearch.core.xcontent.DeprecationHandler;
import org.opensearch.core.xcontent.NamedXContentRegistry;
import org.opensearch.core.xcontent.XContentParser;
import org.opensearch.search.aggregations.AggregationBuilder;
import org.opensearch.search.aggregations.AggregatorFactories;
import org.opensearch.search.aggregations.PipelineAggregationBuilder;
import org.opensearch.search.aggregations.bucket.histogram.HistogramAggregationBuilder;
import org.opensearch.search.aggregations.bucket.terms.TermsAggregationBuilder;
import org.opensearch.search.aggregations.metrics.AvgAggregationBuilder;
import org.opensearch.search.aggregations.metrics.SumAggregationBuilder;
import org.opensearch.search.sort.FieldSortBuilder;

import java.util.List;

/**
 * Prints what OpenSearch 3.10.0 makes of the bodies of {@code derivative}, {@code cumulative_sum} and
 * {@code bucket_sort} (the builder's fields, or the exception) and what request validation says of each placed
 * at the top level, under a {@code terms}, and under a {@code histogram}. Compiled as {@code PipelineRecordings.java} is.
 */
public final class PipelineSerialParse {
    static String chain(Throwable e) {
        Throwable root = e;
        String chain = e.getClass().getName() + ": " + e.getMessage();
        while (root.getCause() != null) { root = root.getCause(); chain += " <- " + root.getClass().getName() + ": " + root.getMessage(); }
        return chain.replace('\n', ' ');
    }

    static void parse(String type, String body) {
        try (XContentParser parser = JsonXContent.jsonXContent.createParser(
            NamedXContentRegistry.EMPTY, DeprecationHandler.THROW_UNSUPPORTED_OPERATION, body)) {
            parser.nextToken();
            PipelineAggregationBuilder b = switch (type) {
                case "serial_diff" -> SerialDiffPipelineAggregationBuilder.parse("p", parser);
                case "cumulative_sum" -> CumulativeSumPipelineAggregationBuilder.PARSER.parse(parser, "p");
                default -> BucketSortPipelineAggregationBuilder.parse("p", parser);
            };
            String shown = org.opensearch.core.common.Strings.toString(org.opensearch.common.xcontent.XContentType.JSON, b);
            System.out.println(type + " " + body + " -> ok " + shown);
        } catch (Exception e) {
            System.out.println(type + " " + body + " -> " + chain(e));
        }
    }

    static void check(String label, AggregatorFactories.Builder root) {
        try {
            ActionRequestValidationException e = root.validate(null);
            System.out.println("validate " + label + " " + (e == null ? "ok" : String.join(" | ", e.validationErrors())));
        } catch (Exception e) {
            System.out.println("validate " + label + " " + chain(e));
        }
    }

    static HistogramAggregationBuilder histogram(PipelineAggregationBuilder... pipes) {
        HistogramAggregationBuilder h = new HistogramAggregationBuilder("h").field("n").interval(10)
            .subAggregation(new SumAggregationBuilder("s").field("n")).subAggregation(new AvgAggregationBuilder("a").field("n"));
        for (PipelineAggregationBuilder p : pipes) { h.subAggregation(p); }
        return h;
    }

    static TermsAggregationBuilder terms(PipelineAggregationBuilder... pipes) {
        TermsAggregationBuilder t = new TermsAggregationBuilder("t").field("f").subAggregation(new SumAggregationBuilder("s").field("n"));
        for (PipelineAggregationBuilder p : pipes) { t.subAggregation(p); }
        return t;
    }

    static AggregatorFactories.Builder one(AggregationBuilder agg) { return AggregatorFactories.builder().addAggregator(agg); }

    public static void main(String[] args) {
        for (String body : new String[] {
            "{}", "{\"buckets_path\":\"s\"}", "{\"buckets_path\":1}", "{\"buckets_path\":\"s\",\"gap_policy\":\"insert_zeros\"}", "{\"buckets_path\":\"s\",\"gap_policy\":\"nope\"}",
            "{\"buckets_path\":\"s\",\"gap_policy\":\"SKIP\"}", "{\"buckets_path\":\"s\",\"gap_policy\":1}",
            "{\"buckets_path\":\"s\",\"lag\":2}", "{\"buckets_path\":\"s\",\"lag\":0}", "{\"buckets_path\":\"s\",\"lag\":-1}", "{\"buckets_path\":\"s\",\"lag\":\"2\"}",
            "{\"buckets_path\":\"s\",\"lag\":1.5}", "{\"buckets_path\":\"s\",\"lag\":null}", "{\"buckets_path\":\"s\",\"lag\":[1]}", "{\"buckets_path\":\"s\",\"lag\":true}",
            "{\"buckets_path\":\"s\",\"lag\":3000000000}",
            "{\"buckets_path\":\"s\",\"format\":\"0.0\"}", "{\"buckets_path\":\"s\",\"format\":1}", "{\"buckets_path\":\"s\",\"unit\":\"1d\"}", "{\"buckets_path\":\"s\",\"foo\":1}",
            "{\"buckets_path\":\"s\",\"foo\":\"x\"}", "{\"buckets_path\":\"s\",\"foo\":[1]}", "{\"buckets_path\":[\"s\"]}", "{\"buckets_path\":[\"s\",\"a\"]}", "{\"buckets_path\":[]}", "{\"buckets_path\":[1]}",
            "{\"buckets_path\":{\"a\":\"s\"}}", "{\"buckets_path\":null}", "{\"lag\":2}"
        }) { parse("serial_diff", body); }
        PipelineAggregationBuilder d = new SerialDiffPipelineAggregationBuilder("p", "s");
        PipelineAggregationBuilder dBad = new SerialDiffPipelineAggregationBuilder("p", "zz");
        PipelineAggregationBuilder dCount = new SerialDiffPipelineAggregationBuilder("p", "_count");
        PipelineAggregationBuilder dKey = new SerialDiffPipelineAggregationBuilder("p", "_key");
        PipelineAggregationBuilder dDeep = new SerialDiffPipelineAggregationBuilder("p", "a>s");
        PipelineAggregationBuilder dSelf = new SerialDiffPipelineAggregationBuilder("p", "p");
        PipelineAggregationBuilder dMetric = new SerialDiffPipelineAggregationBuilder("p", "s.value");
        PipelineAggregationBuilder dNameClash = new SerialDiffPipelineAggregationBuilder("s", "a");
        check("top.serial_diff", AggregatorFactories.builder().addPipelineAggregator(d));
        check("histogram.serial_diff", one(histogram(d)));
        check("histogram.serial_diff.bad", one(histogram(dBad)));
        check("histogram.serial_diff.count", one(histogram(dCount)));
        check("histogram.serial_diff.key", one(histogram(dKey)));
        check("histogram.serial_diff.deep", one(histogram(dDeep)));
        check("histogram.serial_diff.self", one(histogram(dSelf)));
        check("histogram.serial_diff.metric", one(histogram(dMetric)));
        check("histogram.serial_diff.nameclash", one(histogram(dNameClash)));
        check("terms.serial_diff", one(terms(d)));
        check("histogram.serial_diff.after_derivative", one(histogram(new DerivativePipelineAggregationBuilder("q", "s"), new SerialDiffPipelineAggregationBuilder("p", "q"))));
        check("histogram.dup", one(histogram(d, new SerialDiffPipelineAggregationBuilder("p", "s"))));
    }
}

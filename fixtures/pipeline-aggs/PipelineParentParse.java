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
public final class PipelineParentParse {
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
                case "derivative" -> DerivativePipelineAggregationBuilder.parse("p", parser);
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
            "{\"buckets_path\":\"s\",\"format\":\"0.0\"}", "{\"buckets_path\":\"s\",\"unit\":\"1d\"}", "{\"buckets_path\":\"s\",\"foo\":1}", "{\"buckets_path\":[\"s\"]}", "{\"buckets_path\":[\"s\",\"a\"]}",
            "{\"buckets_path\":\"s\",\"foo\":\"x\"}", "{\"buckets_path\":\"s\",\"foo\":[1]}", "{\"buckets_path\":{\"a\":\"s\"}}", "{\"buckets_path\":[]}", "{\"buckets_path\":[1]}",
            "{\"buckets_path\":\"s\",\"gap_policy\":1}", "{\"buckets_path\":\"s\",\"unit\":1}", "{\"buckets_path\":\"s\",\"format\":1}", "{\"buckets_path\":null}"
        }) { parse("derivative", body); parse("cumulative_sum", body); }
        for (String body : new String[] {
            "{}", "{\"from\":1}", "{\"size\":2}", "{\"size\":0}", "{\"from\":-1}", "{\"sort\":[]}", "{\"sort\":\"s\"}", "{\"sort\":[\"s\"]}", "{\"sort\":[{\"s\":\"desc\"}]}",
            "{\"sort\":[{\"s\":{\"order\":\"desc\"}}]}", "{\"sort\":{\"s\":{\"order\":\"desc\"}}}", "{\"sort\":[{\"s\":{\"order\":\"desc\",\"missing\":\"_last\"}}]}",
            "{\"sort\":[{\"s\":{\"order\":\"sideways\"}}]}", "{\"sort\":[{\"_count\":\"asc\"},{\"s\":\"desc\"}]}", "{\"gap_policy\":\"insert_zeros\",\"size\":1}", "{\"size\":1,\"foo\":1}",
            "{\"size\":\"2\"}", "{\"size\":1.5}", "{\"from\":\"1\",\"size\":1}", "{\"size\":null}", "{\"sort\":[{\"s\":\"desc\",\"a\":\"asc\"}]}", "{\"sort\":[1]}", "{\"sort\":[\"s\",\"s\"]}",
            "{\"sort\":[\"s\"],\"size\":0}", "{\"gap_policy\":\"nope\"}", "{\"sort\":[\"s\"],\"gap_policy\":\"nope\"}", "{\"from\":-1,\"size\":0}", "{\"sort\":[{\"s\":\"sideways\"}]}", "{\"sort\":[{\"s\":1}]}",
            "{\"sort\":[\"s\"],\"from\":-1}", "{\"size\":1,\"sort\":[\"s\"],\"from\":1}", "{\"gap_policy\":\"SKIP\"}", "{\"sort\":null}", "{\"sort\":[null]}", "{\"from\":null}", "{\"from\":1.5}", "{\"sort\":[{}]}", "{\"sort\":[\"\"]}", "{\"sort\":[[\"s\"]]}",
            "{\"from\":true}", "{\"sort\":{\"s\":\"desc\"}}", "{\"sort\":[{\"s\":\"DESC\"}]}", "{\"sort\":[{\"s\":\"Asc\"}]}", "{\"sort\":[{\"s\":null}]}", "{\"sort\":[{\"s\":[\"desc\"]}]}", "{\"sort\":{\"s\":\"desc\",\"a\":\"asc\"}}", "{\"sort\":{}}", "{\"gap_policy\":\"Insert_Zeros\"}", "{\"gap_policy\":\"INSERT_ZEROS\"}", "{\"gap_policy\":1}", "{\"gap_policy\":null}", "{\"size\":\"x\"}", "{\"sort\":[\"s\"],\"size\":\"x\"}", "{\"from\":[1]}", "{\"sort\":[{\"s\":\"desc\"},\"a\"],\"from\":1,\"size\":3}"
        }) { parse("bucket_sort", body); }
        PipelineAggregationBuilder d = new DerivativePipelineAggregationBuilder("p", "s");
        PipelineAggregationBuilder dBad = new DerivativePipelineAggregationBuilder("p", "zz");
        PipelineAggregationBuilder c = new CumulativeSumPipelineAggregationBuilder("p", "s");
        PipelineAggregationBuilder cAvgPath = new CumulativeSumPipelineAggregationBuilder("p", "a");
        PipelineAggregationBuilder bs = new BucketSortPipelineAggregationBuilder("p", List.of(new FieldSortBuilder("s")));
        PipelineAggregationBuilder bsBad = new BucketSortPipelineAggregationBuilder("p", List.of(new FieldSortBuilder("zz")));
        PipelineAggregationBuilder bsNone = new BucketSortPipelineAggregationBuilder("p", List.of()).size(1);
        PipelineAggregationBuilder dCount = new DerivativePipelineAggregationBuilder("p", "_count");
        PipelineAggregationBuilder dKey = new DerivativePipelineAggregationBuilder("p", "_key");
        PipelineAggregationBuilder dDeep = new DerivativePipelineAggregationBuilder("p", "a>s");
        PipelineAggregationBuilder dSelf = new DerivativePipelineAggregationBuilder("p", "p");
        PipelineAggregationBuilder dMetric = new DerivativePipelineAggregationBuilder("p", "s.value");
        PipelineAggregationBuilder dNameClash = new DerivativePipelineAggregationBuilder("s", "a");
        check("top.derivative", AggregatorFactories.builder().addPipelineAggregator(d));
        check("top.cumulative_sum", AggregatorFactories.builder().addPipelineAggregator(c));
        check("top.bucket_sort", AggregatorFactories.builder().addPipelineAggregator(bs));
        check("histogram.derivative", one(histogram(d)));
        check("histogram.derivative.bad", one(histogram(dBad)));
        check("histogram.derivative.count", one(histogram(dCount)));
        check("histogram.derivative.key", one(histogram(dKey)));
        check("histogram.derivative.deep", one(histogram(dDeep)));
        check("histogram.derivative.self", one(histogram(dSelf)));
        check("histogram.derivative.metric", one(histogram(dMetric)));
        check("histogram.derivative.nameclash", one(histogram(dNameClash)));
        check("histogram.cumulative_sum", one(histogram(c)));
        check("histogram.cumulative_sum.bad", one(histogram(new CumulativeSumPipelineAggregationBuilder("p", "zz"))));
        check("histogram.cumulative_sum.avgpath", one(histogram(cAvgPath)));
        check("histogram.bucket_sort", one(histogram(bs)));
        check("histogram.bucket_sort.bad", one(histogram(bsBad)));
        check("histogram.bucket_sort.none", one(histogram(bsNone)));
        check("terms.derivative", one(terms(d)));
        check("terms.cumulative_sum", one(terms(c)));
        check("terms.bucket_sort", one(terms(bs)));
        check("terms.bucket_sort.bad", one(terms(bsBad)));
        check("histogram.two", one(histogram(d, new CumulativeSumPipelineAggregationBuilder("q", "p"))));
        check("histogram.dup", one(histogram(d, new CumulativeSumPipelineAggregationBuilder("p", "s"))));
    }
}

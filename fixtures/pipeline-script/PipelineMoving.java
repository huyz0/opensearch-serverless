package org.opensearch.search.aggregations.pipeline;

import org.opensearch.common.settings.Settings;
import org.opensearch.common.util.BigArrays;
import org.opensearch.common.xcontent.XContentFactory;
import org.opensearch.common.xcontent.json.JsonXContent;
import org.opensearch.core.common.bytes.BytesReference;
import org.opensearch.core.xcontent.DeprecationHandler;
import org.opensearch.core.xcontent.NamedXContentRegistry;
import org.opensearch.core.xcontent.ToXContent;
import org.opensearch.core.xcontent.XContentBuilder;
import org.opensearch.core.xcontent.XContentParser;
import org.opensearch.script.Script;
import org.opensearch.script.ScriptContext;
import org.opensearch.script.ScriptEngine;
import org.opensearch.script.ScriptModule;
import org.opensearch.script.ScriptService;
import org.opensearch.script.ScriptType;
import org.opensearch.search.aggregations.InternalAggregation;
import org.opensearch.search.aggregations.bucket.histogram.InternalHistogram;

import java.util.Map;
import java.util.Set;

/**
 * Prints what OpenSearch 3.10.0 does with {@code moving_fn}: how its body parses (and fails) and the body it
 * writes reduced over the four-bucket {@code histogram} of {@link PipelineParent}, with a test script engine
 * whose scripts are named by their source and call {@link MovingFunctions} as a Painless script does:
 * {@code java org.opensearch.search.aggregations.pipeline.PipelineMoving > java-moving-3.10.0.txt}.
 */
public final class PipelineMoving {
    static final class Engine implements ScriptEngine {
        public String getType() { return "test"; }

        @SuppressWarnings("unchecked")
        public <F> F compile(String name, String code, ScriptContext<F> context, Map<String, String> options) {
            MovingFunctionScript.Factory f = () -> new MovingFunctionScript() {
                public double execute(Map<String, Object> params, double[] values) {
                    return switch (code) {
                        case "avg" -> MovingFunctions.unweightedAvg(values);
                        case "sum" -> MovingFunctions.sum(values);
                        case "max" -> MovingFunctions.max(values);
                        case "min" -> MovingFunctions.min(values);
                        case "lw" -> MovingFunctions.linearWeightedAvg(values);
                        case "std" -> MovingFunctions.stdDev(values, MovingFunctions.unweightedAvg(values));
                        case "ewma" -> MovingFunctions.ewma(values, 0.5);
                        case "holt" -> MovingFunctions.holt(values, 0.5, 0.5);
                        case "count" -> values.length;
                        case "params" -> ((Number) params.get("k")).doubleValue() + values.length;
                        case "boom" -> throw new IllegalStateException("boom");
                        default -> throw new IllegalArgumentException("no script " + code);
                    };
                }
            };
            return (F) f;
        }

        public Set<ScriptContext<?>> getSupportedContexts() { return Set.of(MovingFunctionScript.CONTEXT); }
    }

    static final ScriptService SERVICE = new ScriptService(Settings.EMPTY, Map.of("test", new Engine()), ScriptModule.CORE_CONTEXTS);

    static String chain(Throwable e) {
        Throwable root = e;
        String chain = e.getClass().getName() + ": " + e.getMessage();
        while (root.getCause() != null) { root = root.getCause(); chain += " <- " + root.getClass().getName() + ": " + root.getMessage(); }
        return chain.replace('\n', ' ');
    }

    static Script script(String code) { return new Script(ScriptType.INLINE, "test", code, Map.of("k", 3)); }

    static void run(String name, MovFnPipelineAggregationBuilder builder, InternalHistogram input) {
        try {
            PipelineAggregator aggregator = builder.create();
            InternalAggregation.ReduceContext context = InternalAggregation.ReduceContext.forFinalReduction(
                BigArrays.NON_RECYCLING_INSTANCE, SERVICE, i -> {}, PipelineAggregator.PipelineTree.EMPTY);
            InternalAggregation result = aggregator.reduce(input, context);
            XContentBuilder x = XContentFactory.jsonBuilder();
            x.startObject();
            result.toXContent(x, ToXContent.EMPTY_PARAMS);
            x.endObject();
            System.out.println(name + " " + BytesReference.bytes(x).utf8ToString());
        } catch (Exception e) {
            System.out.println(name + " " + chain(e));
        }
    }

    static void parse(String body) {
        try (XContentParser parser = JsonXContent.jsonXContent.createParser(
            NamedXContentRegistry.EMPTY, DeprecationHandler.THROW_UNSUPPORTED_OPERATION, body)) {
            parser.nextToken();
            MovFnPipelineAggregationBuilder b = MovFnPipelineAggregationBuilder.PARSER.parse(parser, "p");
            String shown = org.opensearch.core.common.Strings.toString(org.opensearch.common.xcontent.XContentType.JSON, b);
            System.out.println("parse " + body + " -> ok " + shown);
        } catch (Exception e) {
            System.out.println("parse " + body + " -> " + chain(e));
        }
    }

    public static void main(String[] args) {
        String[] codes = {"avg", "sum", "max", "min", "lw", "std", "ewma", "holt", "count", "params"};
        String[][] inputs = {{"", "plain"}, {"gap", "gap"}, {"empty", "empty"}};
        for (String[] in : inputs) {
            InternalHistogram h = PipelineParent.histogram(in[0].equals("gap"), in[0].equals("empty"));
            for (BucketHelpers.GapPolicy policy : BucketHelpers.GapPolicy.values()) {
                String p = policy.name().toLowerCase(java.util.Locale.ROOT);
                for (String path : new String[] {"s", "a", "_count"}) {
                    for (int window : new int[] {1, 2, 3, 10}) {
                        for (int shift : new int[] {0, 1, -1}) {
                            if (window == 3 && shift != 0 || window == 10 && shift != 0) { continue; }
                            for (String code : codes) {
                                if (!(code.equals("avg") || code.equals("count") || window <= 3 && path.equals("s") || code.equals("params") && window == 2)) { continue; }
                                MovFnPipelineAggregationBuilder b = new MovFnPipelineAggregationBuilder("p", path, script(code), window);
                                b.setShift(shift);
                                b.gapPolicy(policy);
                                run(in[1] + "." + path + "." + p + ".w" + window + ".s" + shift + "." + code, b, h);
                            }
                        }
                    }
                }
            }
        }
        InternalHistogram h = PipelineParent.histogram(false);
        run("boom", new MovFnPipelineAggregationBuilder("p", "s", script("boom"), 2), h);
        run("unknown", new MovFnPipelineAggregationBuilder("p", "s", script("nope"), 2), h);
        run("badpath", new MovFnPipelineAggregationBuilder("p", "zz", script("avg"), 2), h);
        for (String body : new String[] {
            "{\"buckets_path\":\"s\",\"window\":2,\"script\":\"avg\"}",
            "{\"buckets_path\":\"s\",\"window\":2,\"script\":{\"source\":\"avg\",\"lang\":\"test\",\"params\":{\"k\":1}}}",
            "{\"buckets_path\":\"s\",\"window\":2,\"script\":{\"id\":\"stored\"}}",
            "{\"buckets_path\":\"s\",\"window\":2,\"script\":\"avg\",\"shift\":-3}",
            "{\"buckets_path\":\"s\",\"window\":2,\"script\":\"avg\",\"shift\":2.7}",
            "{\"buckets_path\":\"s\",\"window\":2,\"script\":\"avg\",\"gap_policy\":\"insert_zeros\"}",
            "{\"buckets_path\":\"s\",\"window\":2,\"script\":\"avg\",\"gap_policy\":\"nope\"}",
            "{\"buckets_path\":\"s\",\"window\":2,\"script\":\"avg\",\"format\":\"0.0\"}",
            "{\"buckets_path\":\"s\",\"window\":0,\"script\":\"avg\"}",
            "{\"buckets_path\":\"s\",\"window\":-1,\"script\":\"avg\"}",
            "{\"buckets_path\":\"s\",\"window\":\"2\",\"script\":\"avg\"}",
            "{\"buckets_path\":\"s\",\"window\":2.5,\"script\":\"avg\"}",
            "{\"buckets_path\":\"s\",\"script\":\"avg\"}",
            "{\"window\":2,\"script\":\"avg\"}",
            "{\"buckets_path\":\"s\",\"window\":2}",
            "{}",
            "{\"buckets_path\":[\"s\"],\"window\":2,\"script\":\"avg\"}",
            "{\"buckets_path\":{\"x\":\"s\"},\"window\":2,\"script\":\"avg\"}",
            "{\"buckets_path\":\"s\",\"window\":2,\"script\":\"avg\",\"foo\":1}",
            "{\"buckets_path\":\"s\",\"window\":null,\"script\":\"avg\"}",
            "{\"buckets_path\":null,\"window\":2,\"script\":\"avg\"}",
            "{\"buckets_path\":\"s\",\"window\":2,\"script\":1}",
            "{\"buckets_path\":\"s\",\"window\":3000000000,\"script\":\"avg\"}",
            "{\"buckets_path\":\"a>s\",\"window\":2,\"script\":\"avg\"}",
            "{\"buckets_path\":\"s\",\"window\":2,\"script\":{\"source\":\"avg\",\"lang\":\"expression\"}}"
        }) { parse(body); }
    }
}

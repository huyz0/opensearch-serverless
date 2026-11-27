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
import org.opensearch.script.BucketAggregationScript;
import org.opensearch.script.BucketAggregationSelectorScript;
import org.opensearch.script.Script;
import org.opensearch.script.ScriptContext;
import org.opensearch.script.ScriptEngine;
import org.opensearch.script.ScriptModule;
import org.opensearch.script.ScriptService;
import org.opensearch.script.ScriptType;
import org.opensearch.search.aggregations.InternalAggregation;
import org.opensearch.search.aggregations.PipelineAggregationBuilder;
import org.opensearch.search.aggregations.bucket.histogram.InternalHistogram;

import java.util.LinkedHashMap;
import java.util.Map;
import java.util.Set;

/**
 * Prints what OpenSearch 3.10.0 does with {@code bucket_script} and {@code bucket_selector}: how their
 * bodies parse (and fail) and the body each writes reduced over the four-bucket {@code histogram} of
 * {@link PipelineParent}, with a test script engine whose scripts are named by their source (the
 * Painless runs are the script engine's, not the pipeline's):
 * {@code java org.opensearch.search.aggregations.pipeline.PipelineScript > java-script-3.10.0.txt}.
 */
public final class PipelineScript {
    /** A script engine: the source names what it does with {@code params}. */
    static final class Engine implements ScriptEngine {
        public String getType() { return "test"; }

        @SuppressWarnings("unchecked")
        public <F> F compile(String name, String code, ScriptContext<F> context, Map<String, String> options) {
            if (context.equals(BucketAggregationScript.CONTEXT)) {
                BucketAggregationScript.Factory f = params -> new BucketAggregationScript(params) {
                    public Number execute() {
                        Map<String, Object> p = getParams();
                        return switch (code) {
                            case "sum" -> ((Number) p.get("x")).doubleValue() + ((Number) p.get("y")).doubleValue();
                            case "ratio" -> ((Number) p.get("x")).doubleValue() / ((Number) p.get("y")).doubleValue();
                            case "scaled" -> ((Number) p.get("x")).doubleValue() * ((Number) p.get("k")).doubleValue();
                            case "int" -> 7;
                            case "long" -> 5000000000L;
                            case "nan" -> Double.NaN;
                            case "null" -> null;
                            case "x" -> (Number) p.get("x");
                            default -> throw new IllegalArgumentException("no script " + code);
                        };
                    }
                };
                return (F) f;
            }
            BucketAggregationSelectorScript.Factory f = params -> new BucketAggregationSelectorScript(params) {
                public boolean execute() {
                    Map<String, Object> p = getParams();
                    return switch (code) {
                        case "gt10" -> ((Number) p.get("x")).doubleValue() > 10;
                        case "true" -> true;
                        case "false" -> false;
                        case "countgt3" -> ((Number) p.get("y")).doubleValue() > 3;
                        default -> throw new IllegalArgumentException("no script " + code);
                    };
                }
            };
            return (F) f;
        }

        public Set<ScriptContext<?>> getSupportedContexts() {
            return Set.of(BucketAggregationScript.CONTEXT, BucketAggregationSelectorScript.CONTEXT);
        }
    }

    static final ScriptService SERVICE = new ScriptService(Settings.EMPTY, Map.of("test", new Engine()), ScriptModule.CORE_CONTEXTS);

    static String chain(Throwable e) {
        Throwable root = e;
        String chain = e.getClass().getName() + ": " + e.getMessage();
        while (root.getCause() != null) { root = root.getCause(); chain += " <- " + root.getClass().getName() + ": " + root.getMessage(); }
        return chain.replace('\n', ' ');
    }

    static Map<String, String> paths(String... pairs) {
        Map<String, String> m = new LinkedHashMap<>();
        for (int i = 0; i < pairs.length; i += 2) { m.put(pairs[i], pairs[i + 1]); }
        return m;
    }

    static Script script(String code) {
        return new Script(ScriptType.INLINE, "test", code, Map.of("k", 3));
    }

    static void run(String name, PipelineAggregationBuilder builder, InternalHistogram input) {
        try {
            PipelineAggregator aggregator = ((AbstractPipelineAggregationBuilder<?>) builder).create();
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

    static void parse(String type, String body) {
        try (XContentParser parser = JsonXContent.jsonXContent.createParser(
            NamedXContentRegistry.EMPTY, DeprecationHandler.THROW_UNSUPPORTED_OPERATION, body)) {
            parser.nextToken();
            PipelineAggregationBuilder b = type.equals("bucket_script")
                ? BucketScriptPipelineAggregationBuilder.PARSER.parse(parser, "p")
                : BucketSelectorPipelineAggregationBuilder.parse("p", parser);
            String shown = org.opensearch.core.common.Strings.toString(org.opensearch.common.xcontent.XContentType.JSON, b);
            System.out.println("parse " + type + " " + body + " -> ok " + shown);
        } catch (Exception e) {
            System.out.println("parse " + type + " " + body + " -> " + chain(e));
        }
    }

    public static void main(String[] args) throws Exception {
        for (boolean gap : new boolean[] {false, true}) {
            String g = gap ? "gap." : "";
            for (BucketHelpers.GapPolicy policy : BucketHelpers.GapPolicy.values()) {
                String p = policy.name().toLowerCase(java.util.Locale.ROOT);
                for (String path : new String[] {"s", "a", "_count"}) {
                    run(g + "script.sum." + path + "." + p, new BucketScriptPipelineAggregationBuilder("p", paths("x", path, "y", "_count"), script("sum")).gapPolicy(policy), PipelineParent.histogram(gap));
                    run(g + "select.gt10." + path + "." + p, new BucketSelectorPipelineAggregationBuilder("p", paths("x", path), script("gt10")).gapPolicy(policy), PipelineParent.histogram(gap));
                }
                run("empty.script.sum.s." + p, new BucketScriptPipelineAggregationBuilder("p", paths("x", "s", "y", "_count"), script("sum")).gapPolicy(policy), PipelineParent.histogram(false, true));
                run("empty.select.gt10.s." + p, new BucketSelectorPipelineAggregationBuilder("p", paths("x", "s"), script("gt10")).gapPolicy(policy), PipelineParent.histogram(false, true));
            }
        }
        run("script.ratio", new BucketScriptPipelineAggregationBuilder("p", paths("x", "s", "y", "_count"), script("ratio")), PipelineParent.histogram(false));
        run("script.ratio.empty", new BucketScriptPipelineAggregationBuilder("p", paths("x", "s", "y", "_count"), script("ratio")).gapPolicy(BucketHelpers.GapPolicy.INSERT_ZEROS), PipelineParent.histogram(false, true));
        run("script.params", new BucketScriptPipelineAggregationBuilder("p", paths("x", "s"), script("scaled")), PipelineParent.histogram(false));
        run("script.int", new BucketScriptPipelineAggregationBuilder("p", paths("x", "s"), script("int")), PipelineParent.histogram(false));
        run("script.long", new BucketScriptPipelineAggregationBuilder("p", paths("x", "s"), script("long")), PipelineParent.histogram(false));
        run("script.nan", new BucketScriptPipelineAggregationBuilder("p", paths("x", "s"), script("nan")), PipelineParent.histogram(false));
        run("script.null", new BucketScriptPipelineAggregationBuilder("p", paths("x", "s"), script("null")), PipelineParent.histogram(false));
        run("script.nopath", new BucketScriptPipelineAggregationBuilder("p", paths(), script("int")), PipelineParent.histogram(false));
        run("script.badpath", new BucketScriptPipelineAggregationBuilder("p", paths("x", "zz"), script("x")), PipelineParent.histogram(false));
        run("script.metric", new BucketScriptPipelineAggregationBuilder("p", paths("x", "s.value"), script("x")), PipelineParent.histogram(false));
        run("script.avgmetric", new BucketScriptPipelineAggregationBuilder("p", paths("x", "a.value"), script("x")), PipelineParent.histogram(false));
        run("script.format", new BucketScriptPipelineAggregationBuilder("p", paths("x", "s"), script("x")).format("0.0"), PipelineParent.histogram(false));
        run("select.true", new BucketSelectorPipelineAggregationBuilder("p", paths("x", "s"), script("true")), PipelineParent.histogram(false));
        run("select.false", new BucketSelectorPipelineAggregationBuilder("p", paths("x", "s"), script("false")), PipelineParent.histogram(false));
        run("select.count", new BucketSelectorPipelineAggregationBuilder("p", paths("y", "_count"), script("countgt3")), PipelineParent.histogram(false));
        run("select.nopath", new BucketSelectorPipelineAggregationBuilder("p", paths(), script("true")), PipelineParent.histogram(false));
        String inline = "{\"source\":\"sum\",\"lang\":\"test\"}";
        for (String type : new String[] {"bucket_script", "bucket_selector"}) {
            for (String body : new String[] {
                "{\"buckets_path\":{\"x\":\"s\",\"y\":\"_count\"},\"script\":\"x + y\"}",
                "{\"buckets_path\":{\"x\":\"s\"},\"script\":" + inline + "}",
                "{\"buckets_path\":{\"x\":\"s\"},\"script\":{\"source\":\"a\",\"lang\":\"painless\",\"params\":{\"k\":1}}}",
                "{\"buckets_path\":{\"x\":\"s\"},\"script\":{\"id\":\"stored\"}}",
                "{\"buckets_path\":[\"s\",\"_count\"],\"script\":\"a\"}",
                "{\"buckets_path\":\"s\",\"script\":\"a\"}",
                "{\"buckets_path\":{\"x\":\"s\"},\"script\":\"a\",\"gap_policy\":\"insert_zeros\"}",
                "{\"buckets_path\":{\"x\":\"s\"},\"script\":\"a\",\"gap_policy\":\"skip\"}",
                "{\"buckets_path\":{\"x\":\"s\"},\"script\":\"a\",\"gap_policy\":\"nope\"}",
                "{\"buckets_path\":{\"x\":\"s\"},\"script\":\"a\",\"format\":\"0.0\"}",
                "{}",
                "{\"gap_policy\":\"skip\"}",
                "{\"buckets_path\":{\"x\":\"s\"}}",
                "{\"script\":\"a\"}",
                "{\"buckets_path\":{},\"script\":\"a\"}",
                "{\"buckets_path\":{\"x\":1},\"script\":\"a\"}",
                "{\"buckets_path\":{\"x\":\"s\"},\"script\":1}",
                "{\"buckets_path\":{\"x\":\"s\"},\"script\":\"a\",\"foo\":1}",
                "{\"buckets_path\":null,\"script\":\"a\"}",
                "{\"buckets_path\":{\"x\":\"s\"},\"script\":null}",
                "{\"buckets_path\":{\"x\":null},\"script\":\"a\"}",
                "{\"buckets_path\":{\"x\":\"s\"},\"script\":\"\"}",
                "{\"buckets_path\":{\"x\":\"s\"},\"script\":{\"source\":\"a\",\"lang\":\"expression\"}}",
                "{\"buckets_path\":{\"x\":\"s\"},\"script\":{\"source\":\"a\",\"params\":1}}",
                "{\"buckets_path\":{\"x\":\"s\",\"y\":\"a>s\"},\"script\":\"a\"}"
            }) { parse(type, body); }
        }
    }
}

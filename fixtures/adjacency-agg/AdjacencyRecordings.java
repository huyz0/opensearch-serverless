package org.opensearch.search.aggregations.bucket.adjacency;

import org.opensearch.common.util.BigArrays;
import org.opensearch.common.xcontent.XContentFactory;
import org.opensearch.common.xcontent.XContentType;
import org.opensearch.common.xcontent.json.JsonXContent;
import org.opensearch.core.common.bytes.BytesReference;
import org.opensearch.core.xcontent.DeprecationHandler;
import org.opensearch.core.xcontent.NamedXContentRegistry;
import org.opensearch.core.xcontent.ToXContent;
import org.opensearch.core.xcontent.XContentBuilder;
import org.opensearch.core.xcontent.XContentParser;
import org.opensearch.search.aggregations.AggregationBuilder;
import org.opensearch.search.aggregations.InternalAggregation;
import org.opensearch.search.aggregations.InternalAggregations;

import java.util.ArrayList;
import java.util.List;
import java.util.Map;

/**
 * Prints what OpenSearch 3.10.0 does with an {@code adjacency_matrix}: how bodies parse (and fail),
 * how shard results reduce and how the result is written, one {@code name text} line each:
 * {@code java org.opensearch.search.aggregations.bucket.adjacency.AdjacencyRecordings > java-adjacency-3.10.0.txt}.
 */
public final class AdjacencyRecordings {
    static final NamedXContentRegistry REGISTRY = new NamedXContentRegistry(new org.opensearch.search.SearchModule(org.opensearch.common.settings.Settings.EMPTY, List.of()).getNamedXContents());

    static String chain(Throwable e) {
        Throwable root = e;
        String chain = e.getClass().getName() + ": " + e.getMessage();
        while (root.getCause() != null) { root = root.getCause(); chain += " <- " + root.getClass().getName() + ": " + root.getMessage(); }
        return chain.replace('\n', ' ');
    }

    static void parse(String body) {
        try (XContentParser parser = JsonXContent.jsonXContent.createParser(
            REGISTRY, DeprecationHandler.THROW_UNSUPPORTED_OPERATION, body)) {
            parser.nextToken();
            AggregationBuilder b = AdjacencyMatrixAggregationBuilder.parse(parser, "p");
            AdjacencyMatrixAggregationBuilder a = (AdjacencyMatrixAggregationBuilder) b;
            System.out.println("parse " + body + " -> ok separator=" + a.separator() + " only=" + a.isShowOnlyIntersecting() + " keys=" + a.filters().keySet());
        } catch (Exception e) {
            System.out.println("parse " + body + " -> " + chain(e));
        }
    }

    static InternalAdjacencyMatrix shard(Object... keyCount) {
        List<InternalAdjacencyMatrix.InternalBucket> buckets = new ArrayList<>();
        for (int i = 0; i < keyCount.length; i += 2) {
            buckets.add(new InternalAdjacencyMatrix.InternalBucket((String) keyCount[i], ((Number) keyCount[i + 1]).longValue(), InternalAggregations.EMPTY));
        }
        return new InternalAdjacencyMatrix("p", buckets, null);
    }

    static void show(String name, InternalAggregation result, boolean typed) throws Exception {
        XContentBuilder x = XContentFactory.jsonBuilder();
        x.startObject();
        result.toXContent(x, typed ? new ToXContent.MapParams(Map.of("typed_keys", "true")) : ToXContent.EMPTY_PARAMS);
        x.endObject();
        System.out.println(name + " " + BytesReference.bytes(x).utf8ToString());
    }

    static void reduce(String name, boolean typed, InternalAdjacencyMatrix... shards) throws Exception {
        InternalAggregation.ReduceContext context = InternalAggregation.ReduceContext.forFinalReduction(
            BigArrays.NON_RECYCLING_INSTANCE, null, i -> {}, org.opensearch.search.aggregations.pipeline.PipelineAggregator.PipelineTree.EMPTY);
        show(name, shards[0].reduce(new ArrayList<InternalAggregation>(List.of(shards)), context), typed);
    }

    public static void main(String[] args) throws Exception {
        String q = "{\"term\":{\"f\":\"x\"}}";
        for (String body : new String[] {
            "{\"filters\":{\"a\":" + q + ",\"b\":" + q + "}}",
            "{\"filters\":{\"b\":" + q + ",\"a\":" + q + "}}",
            "{\"filters\":{\"a\":" + q + ",\"b\":" + q + "},\"separator\":\"|\"}",
            "{\"filters\":{\"a\":" + q + ",\"b\":" + q + "},\"separator\":\"\"}",
            "{\"filters\":{\"a\":" + q + ",\"b\":" + q + "},\"show_only_intersecting\":true}",
            "{\"filters\":{\"a\":" + q + ",\"b\":" + q + "},\"show_only_intersecting\":false}",
            "{\"filters\":{\"a\":" + q + "}}",
            "{\"filters\":{}}", "{}", "{\"separator\":\"&\"}", "{\"filters\":[" + q + "]}", "{\"filters\":null}",
            "{\"filters\":{\"a\":" + q + "},\"foo\":1}", "{\"filters\":{\"a\":" + q + "},\"separator\":1}", "{\"filters\":{\"a\":" + q + "},\"show_only_intersecting\":\"x\"}",
            "{\"filters\":{\"a\":" + q + ",\"b\":" + q + "},\"other_bucket\":true}", "{\"filters\":{\"a\":1}}", "{\"filters\":{\"a\":{}}}", "{\"filters\":\"a\"}",
            "{\"filters\":{\"a\":" + q + "},\"separator\":null}", "{\"filters\":{\"a\":" + q + "},\"separator\":[\"&\"]}",
            "{\"filters\":{\"\":" + q + ",\"b\":" + q + "}}", "{\"filters\":{\"a&b\":" + q + ",\"b\":" + q + "}}"
        }) { parse(body); }
        reduce("reduce.one", false, shard("a", 2, "b", 3, "a&b", 1));
        reduce("reduce.two", false, shard("a", 2, "b", 3, "a&b", 1), shard("a", 5, "c", 1, "a&c", 4));
        reduce("reduce.sorted", false, shard("b", 1, "a&b", 1), shard("a", 1, "a&c", 1, "b&c", 1));
        reduce("reduce.none", false, shard());
        reduce("reduce.typed", true, shard("a", 2, "b", 3, "a&b", 1));
        reduce("reduce.unicode", false, shard("é", 1, "z", 1, "😀", 1, "ﬀ", 1));
        reduce("reduce.numbers", false, shard("10", 1, "9", 1, "a", 1, "A", 1, "_", 1));
        reduce("reduce.sep", false, shard("a", 1, "a|b", 2, "b", 3), shard("a|b", 1));
    }
}

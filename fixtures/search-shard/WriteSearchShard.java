import org.opensearch.common.unit.TimeValue;
import org.opensearch.core.common.bytes.BytesArray;
import org.opensearch.common.io.stream.BytesStreamOutput;
import org.opensearch.core.common.io.stream.Writeable;
import org.opensearch.index.query.QueryBuilders;
import org.opensearch.search.SearchHit;
import org.opensearch.search.builder.SearchSourceBuilder;
import org.opensearch.search.sort.SortOrder;
import org.opensearch.search.aggregations.AggregationBuilders;
import org.opensearch.serverless.forwarding.ForwardedSearchRequest;
import org.opensearch.serverless.forwarding.ForwardedSearchResponse;

import java.util.HexFormat;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;

/**
 * Writes the bytes the Java shell puts on the wire for {@code internal:serverless/search/shard}, one
 * {@code name hex} line each, for the Rust decoder and encoder to be tested against:
 * {@code WriteSearchShard > java-3.10.0.txt}.
 */
public final class WriteSearchShard {
    private static final Map<String, byte[]> OUT = new LinkedHashMap<>();

    private static void request(String name, SearchSourceBuilder source) throws Exception {
        final BytesStreamOutput out = new BytesStreamOutput();
        new ForwardedSearchRequest("idx", 2, source, "uuid-1", 1_700_000_000_000L).writeTo(out);
        OUT.put("request." + name, out.bytes().toBytesRef().bytes.length == out.size() ? out.bytes().toBytesRef().bytes : java.util.Arrays.copyOf(out.bytes().toBytesRef().bytes, out.size()));
    }

    private static SearchHit hit(int doc, String id, float score, String source, long seq) {
        final SearchHit hit = new SearchHit(doc, id, Map.of(), Map.of());
        hit.score(score);
        hit.version(3);
        hit.setSeqNo(seq);
        hit.setPrimaryTerm(2);
        hit.sourceRef(new BytesArray(source));
        return hit;
    }

    private static void response(String name, ForwardedSearchResponse r) throws Exception {
        final BytesStreamOutput out = new BytesStreamOutput();
        r.writeTo(out);
        OUT.put("response." + name, java.util.Arrays.copyOf(out.bytes().toBytesRef().bytes, out.size()));
    }

    public static void main(String[] args) throws Exception {
        request("match_all", new SearchSourceBuilder().query(QueryBuilders.matchAllQuery()));
        request("no_query", new SearchSourceBuilder());
        request("term", new SearchSourceBuilder().query(QueryBuilders.termQuery("n", "a").boost(2.0f).queryName("q")));
        request("term_long", new SearchSourceBuilder().query(QueryBuilders.termQuery("n", 7L)));
        request("terms", new SearchSourceBuilder().query(QueryBuilders.termsQuery("tag", "a", "b")));
        request("range", new SearchSourceBuilder().query(QueryBuilders.rangeQuery("n").gte(3L).lt(9L)));
        request("range_format", new SearchSourceBuilder().query(QueryBuilders.rangeQuery("t").gt("now-1h").format("epoch_millis")));
        request("bool", new SearchSourceBuilder().query(QueryBuilders.boolQuery()
            .must(QueryBuilders.termQuery("a", "x"))
            .filter(QueryBuilders.rangeQuery("n").gte(1L))
            .should(QueryBuilders.existsQuery("f"))
            .mustNot(QueryBuilders.idsQuery().addIds("1", "2"))
            .minimumShouldMatch("1")));
        request("ids", new SearchSourceBuilder().query(QueryBuilders.idsQuery().addIds("a", "b", "c")));
        request("exists", new SearchSourceBuilder().query(QueryBuilders.existsQuery("f")));
        request("options", new SearchSourceBuilder().query(QueryBuilders.matchAllQuery()).from(3).size(5).trackTotalHitsUpTo(100)
            .minScore(0.5f).terminateAfter(7).timeout(TimeValue.timeValueSeconds(2)));
        request("sorted", new SearchSourceBuilder().query(QueryBuilders.matchAllQuery()).sort("n", SortOrder.DESC));
        request("aggs_then_from", new SearchSourceBuilder().aggregation(AggregationBuilders.terms("t").field("tag")).from(4));
        request("match", new SearchSourceBuilder().query(QueryBuilders.matchQuery("title", "x")));

        response("empty", new ForwardedSearchResponse(0, List.of()));
        response("two_hits", new ForwardedSearchResponse(42, List.of(
            hit(0, "a", 1.5f, "{\"n\":1}", 7), hit(1, "b", 0.25f, "{\"n\":2}", 8))));

        final SearchHit named = hit(0, "a", 1.5f, "{\"n\":1}", 7);
        named.matchedQueries(new String[] { "q" });
        response("matched_queries", new ForwardedSearchResponse(1, List.of(named)));
        final SearchHit scored = hit(1, "b", 0.25f, "{\"n\":2}", 8);
        final LinkedHashMap<String, Float> scores = new LinkedHashMap<>();
        scores.put("first", 1.25f);
        scores.put("second", 0.5f);
        scored.matchedQueriesWithScores(scores);
        response("matched_queries_scored", new ForwardedSearchResponse(1, List.of(scored)));

        final HexFormat hex = HexFormat.of();
        for (Map.Entry<String, byte[]> e : OUT.entrySet()) {
            System.out.println(e.getKey() + " " + hex.formatHex(e.getValue()));
        }
    }
}

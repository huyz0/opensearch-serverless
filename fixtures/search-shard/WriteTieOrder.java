import org.opensearch.common.blobstore.BlobPath;
import org.opensearch.common.blobstore.fs.FsBlobStore;
import org.opensearch.common.settings.Settings;
import org.opensearch.core.index.shard.ShardId;
import org.opensearch.common.io.stream.BytesStreamOutput;
import org.opensearch.index.query.QueryBuilders;
import org.opensearch.search.builder.SearchSourceBuilder;
import org.opensearch.serverless.cluster.IndexDescriptor;
import org.opensearch.serverless.forwarding.ForwardedSearchRequest;
import org.opensearch.serverless.forwarding.ForwardedSearchResponse;
import org.opensearch.serverless.metadata.MetadataPlane;
import org.opensearch.serverless.shard.ShardQuery;
import org.opensearch.serverless.shell.ServerlessNode;

import java.nio.file.Files;
import java.util.Arrays;
import java.util.HexFormat;
import java.util.List;
import java.util.Map;

/**
 * Runs the Java shell's own forwarded-search path ({@code ShardQuery.execute}, then
 * {@code ForwardedSearchResponse.writeTo}) over a shard whose tied hits are not in {@code _id} order, and prints
 * one {@code name hex} line per request and answer, for the Rust owner to be compared with:
 * {@code WriteTieOrder > java-ties-3.10.0.txt}.
 *
 * <p>The shard: a refresh after {@code z, m, q}, then {@code m} rewritten, {@code q} deleted, {@code a} and
 * {@code k} added, and a second refresh. Every live hit scores 1.0 under {@code match_all}, so the order the
 * hits come in is the tie order alone: document order is {@code z m a k}, {@code _id} order would be
 * {@code a k m z}.
 *
 * <p>The second shard, {@code scored}, has two published segments and no tail: {@code z m q} in the first and
 * {@code a k y} in the second, every document holding the same word and a different number of others, so
 * {@code match_all} ties at 1.0 across the two segments and a {@code term} on {@code msg} ties below 1.0
 * (three documents of one length, two of another, one of a third).
 */
public final class WriteTieOrder {
    public static void main(String[] args) throws Exception {
        final var dir = Files.createTempDirectory("tie-order");
        try (var store = new FsBlobStore(1024, dir.resolve("blobs"), false)) {
            final var metadata = new MetadataPlane(store, BlobPath.cleanPath(), () -> 1000L, 30_000L);
            final var settings = Settings.builder()
                .put("node.name", "tie-order")
                .put("path.home", dir.resolve("home"))
                .put("network.host", "127.0.0.1")
                .put("http.port", "0")
                .put("transport.port", "0")
                .put("serverless.roles", "ingest,search")
                .build();
            try (var node = new ServerlessNode(settings)) {
                node.startup().start();
                node.setMetadataPlane(metadata);
                metadata.createIndex(new IndexDescriptor("idx", "uuid-1", 1, "{\"properties\":{\"n\":{\"type\":\"integer\"}}}", null));
                final ShardId shard = node.activation().activateWriter(metadata, "idx", 0).orElseThrow();
                for (String id : List.of("z", "m", "q")) {
                    node.writes().index(shard, id, "{\"n\":1}");
                }
                node.reconciler().shard(shard).refresh("tie-order");
                node.writes().index(shard, "m", "{\"n\":2}");
                node.writes().delete(shard, "q");
                node.writes().index(shard, "a", "{\"n\":3}");
                node.writes().index(shard, "k", "{\"n\":4}");
                node.reconciler().shard(shard).refresh("tie-order");

                answer("match_all", node, shard, new SearchSourceBuilder().query(QueryBuilders.matchAllQuery()));
                answer("match_all_size_2", node, shard, new SearchSourceBuilder().query(QueryBuilders.matchAllQuery()).size(2));

                metadata.createIndex(
                    new IndexDescriptor(
                        "scored",
                        "uuid-2",
                        1,
                        "{\"properties\":{\"n\":{\"type\":\"integer\"},\"msg\":{\"type\":\"text\"}}}",
                        null
                    )
                );
                final ShardId scored = node.activation().activateWriter(metadata, "scored", 0).orElseThrow();
                put(node, scored, "z", 1, "alpha");
                put(node, scored, "m", 2, "alpha beta");
                put(node, scored, "q", 3, "alpha");
                node.reconciler().shard(scored).refresh("tie-order");
                put(node, scored, "a", 4, "alpha beta");
                put(node, scored, "k", 5, "alpha gamma delta");
                put(node, scored, "y", 6, "alpha");
                node.reconciler().shard(scored).refresh("tie-order");

                final var all = QueryBuilders.matchAllQuery();
                final var alpha = QueryBuilders.termQuery("msg", "alpha");
                answer("segments_match_all", node, scored, new SearchSourceBuilder().query(all));
                answer("segments_match_all_size_4", node, scored, new SearchSourceBuilder().query(all).size(4));
                answer("scored_term", node, scored, new SearchSourceBuilder().query(alpha));
                answer("scored_term_size_2", node, scored, new SearchSourceBuilder().query(alpha).size(2));
                answer("scored_term_size_4", node, scored, new SearchSourceBuilder().query(alpha).size(4));
            }
        }
    }

    private static void put(ServerlessNode node, ShardId shard, String id, int n, String msg) throws Exception {
        node.writes().index(shard, id, "{\"n\":" + n + ",\"msg\":\"" + msg + "\"}");
    }

    private static void answer(String name, ServerlessNode node, ShardId shard, SearchSourceBuilder source) throws Exception {
        final var request = new BytesStreamOutput();
        new ForwardedSearchRequest("idx", 2, source, "uuid-1", 1_700_000_000_000L).writeTo(request);
        System.out.println("request." + name + " " + HexFormat.of().formatHex(Arrays.copyOf(request.bytes().toBytesRef().bytes, request.size())));
        final var result = ShardQuery.execute(node.searchService(), shard, source.trackTotalHits(true).fetchSource(true), 1_700_000_000_000L);
        final var out = new BytesStreamOutput();
        new ForwardedSearchResponse(result).writeTo(out);
        System.out.println("response." + name + " " + HexFormat.of().formatHex(Arrays.copyOf(out.bytes().toBytesRef().bytes, out.size())));
    }
}

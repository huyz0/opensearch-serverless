package org.opensearch.search.aggregations.bucket.terms;

import org.apache.lucene.util.BytesRef;
import org.opensearch.common.util.BigArrays;
import org.opensearch.common.xcontent.XContentFactory;
import org.opensearch.core.common.bytes.BytesReference;
import org.opensearch.core.xcontent.ToXContent;
import org.opensearch.core.xcontent.XContentBuilder;
import org.opensearch.search.DocValueFormat;
import org.opensearch.search.aggregations.BucketOrder;
import org.opensearch.search.aggregations.InternalAggregation;
import org.opensearch.search.aggregations.InternalAggregations;
import org.opensearch.search.aggregations.pipeline.PipelineAggregator;

import java.util.List;

/**
 * Prints the response body OpenSearch 3.10.0 writes after the final reduce of {@code multi_terms} shard results
 * (a keyword and a long term, ordered by count descending), one {@code name json} line each. Compiled as
 * {@code CardRecordings.java} is, in the aggregation's package for the package-private constructors:
 * {@code java org.opensearch.search.aggregations.bucket.terms.MultiTermsRecordings > java-multi-terms-3.10.0.txt}.
 */
public final class MultiTermsRecordings {
    static final List<DocValueFormat> FORMATS = List.of(DocValueFormat.RAW, DocValueFormat.RAW);

    static InternalMultiTerms.Bucket b(String k, long n, long count) {
        return new InternalMultiTerms.Bucket(List.of(new BytesRef(k), n), count, InternalAggregations.EMPTY, false, 0, FORMATS);
    }

    static InternalMultiTerms shard(List<InternalMultiTerms.Bucket> buckets, int size, int shardSize, long other) {
        BucketOrder order = BucketOrder.compound(BucketOrder.count(false));
        return new InternalMultiTerms("m", order, order, null, shardSize, false, other, 0, FORMATS, buckets,
            new TermsAggregator.BucketCountThresholds(1, 0, size, shardSize));
    }

    static void reduce(String name, InternalMultiTerms... shards) throws Exception {
        InternalAggregation.ReduceContext context = InternalAggregation.ReduceContext.forFinalReduction(
            BigArrays.NON_RECYCLING_INSTANCE, null, i -> {}, PipelineAggregator.PipelineTree.EMPTY);
        InternalAggregation reduced = shards[0].reduce(List.of(shards), context);
        XContentBuilder x = XContentFactory.jsonBuilder();
        x.startObject();
        reduced.toXContent(x, ToXContent.EMPTY_PARAMS);
        x.endObject();
        System.out.println(name + " " + BytesReference.bytes(x).utf8ToString());
    }

    public static void main(String[] args) throws Exception {
        // One shard that cut a bucket and had others: the buckets and documents over `size` join `sum_other_doc_count`.
        reduce("one", shard(List.of(b("a", 1, 3), b("b", 2, 1)), 1, 2, 4));
        // Two shards that each filled their `shard_size`: the error bound is the sum of their smallest counts.
        reduce("two", shard(List.of(b("a", 1, 3), b("b", 2, 1)), 1, 2, 4), shard(List.of(b("a", 1, 2), b("c", 3, 2)), 1, 2, 0));
        reduce("three", shard(List.of(b("a", 1, 3), b("b", 2, 1)), 2, 2, 4), shard(List.of(b("c", 3, 2), b("d", 4, 1)), 2, 2, 0));
        // Equal counts order by key, term by term: text by its bytes, numbers by value.
        reduce("ties_text", shard(List.of(b("b", 1, 5), b("a", 1, 5)), 10, 20, 0));
        reduce("ties_number", shard(List.of(b("a", 10, 5), b("a", 9, 5), b("a", 100, 5)), 10, 20, 0));
        reduce("ties_mixed", shard(List.of(b("a", 1, 3), b("b", 2, 3), b("a", 0, 1)), 10, 20, 0));
        // Equal keys add over shards.
        reduce("merged", shard(List.of(b("a", 1, 3), b("b", 2, 1)), 10, 20, 0), shard(List.of(b("a", 1, 2), b("c", 3, 2)), 10, 20, 0));
    }
}

package org.opensearch.search.aggregations.bucket.terms;

import org.apache.lucene.util.BytesRef;
import org.opensearch.common.io.stream.BytesStreamOutput;
import org.opensearch.search.DocValueFormat;
import org.opensearch.search.aggregations.AggregationBuilders;
import org.opensearch.search.aggregations.AggregatorFactories;
import org.opensearch.search.aggregations.BucketOrder;
import org.opensearch.search.aggregations.InternalAggregation;
import org.opensearch.search.aggregations.InternalAggregations;
import org.opensearch.search.aggregations.metrics.InternalMax;

import java.util.HexFormat;
import java.util.List;

/**
 * Writes the bytes OpenSearch 3.10.0 puts on the wire for forwarded {@code terms} aggregations (the
 * builders, and the shard's {@code StringTerms} and {@code LongTerms}, with and without a
 * sub-aggregation), one {@code name hex} line each. Compiled as {@code CardRecordings.java} is:
 * {@code java org.opensearch.search.aggregations.bucket.terms.TermsRecordings > java-terms-3.10.0.txt}.
 */
public final class TermsRecordings {
    static void line(String name, BytesStreamOutput out) {
        System.out.println(name + " " + HexFormat.of().formatHex(java.util.Arrays.copyOf(out.bytes().toBytesRef().bytes, out.size())));
    }
    static void answer(String name, InternalAggregation agg) throws Exception {
        BytesStreamOutput out = new BytesStreamOutput();
        InternalAggregations.from(List.of(agg)).writeTo(out);
        line(name, out);
    }
    public static void main(String[] a) throws Exception {
        BytesStreamOutput out = new BytesStreamOutput();
        new AggregatorFactories.Builder()
            .addAggregator(AggregationBuilders.terms("t").field("tag"))
            .addAggregator(AggregationBuilders.terms("u").field("n").size(5).minDocCount(2).shardSize(20).order(BucketOrder.key(true)).showTermDocCountError(true))
            .writeTo(out);
        line("request.terms", out);
        out = new BytesStreamOutput();
        new AggregatorFactories.Builder()
            .addAggregator(AggregationBuilders.terms("u").field("n").size(5).minDocCount(2).shardSize(20).order(BucketOrder.key(true)).showTermDocCountError(true)
                .subAggregation(AggregationBuilders.max("m").field("n")))
            .writeTo(out);
        line("request.terms_subs", out);
        InternalAggregations subs = InternalAggregations.from(List.of(new InternalMax("m", 4.0, DocValueFormat.RAW, null)));
        TermsAggregator.BucketCountThresholds th = new TermsAggregator.BucketCountThresholds(1, 0, 10, 50);
        for (boolean withSubs : new boolean[] {false, true}) {
            String sfx = withSubs ? "_subs" : "";
            InternalAggregations in = withSubs ? subs : InternalAggregations.EMPTY;
            StringTerms.Bucket b1 = new StringTerms.Bucket(new BytesRef("a"), 3, in, false, 0, DocValueFormat.RAW);
            StringTerms.Bucket b2 = new StringTerms.Bucket(new BytesRef("b"), 1, InternalAggregations.EMPTY, false, 0, DocValueFormat.RAW);
            answer("response.string_terms" + sfx, new StringTerms("t", BucketOrder.count(false), BucketOrder.count(false), null, DocValueFormat.RAW, 50, false, 2, List.of(b1, b2), 0, th));
            LongTerms.Bucket l1 = new LongTerms.Bucket(7, 3, in, true, 1, DocValueFormat.RAW);
            answer("response.long_terms" + sfx, new LongTerms("u", BucketOrder.key(true), BucketOrder.key(true), null, DocValueFormat.RAW, 20, true, 0, List.of(l1), -1, th));
        }
        answer("response.string_terms_empty", new StringTerms("t", BucketOrder.count(false), BucketOrder.count(false), null, DocValueFormat.RAW, 50, false, 0, List.of(), 0, th));
    }
}

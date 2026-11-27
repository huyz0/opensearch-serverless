package org.opensearch.search.aggregations.bucket.terms;

import org.apache.lucene.util.BytesRef;
import org.opensearch.common.io.stream.BytesStreamOutput;
import org.opensearch.common.util.SetBackedScalingCuckooFilter;
import org.opensearch.search.DocValueFormat;
import org.opensearch.search.aggregations.AggregationBuilder;
import org.opensearch.search.aggregations.AggregationBuilders;
import org.opensearch.search.aggregations.AggregatorFactories;
import org.opensearch.search.aggregations.BucketOrder;
import org.opensearch.search.aggregations.InternalAggregation;
import org.opensearch.search.aggregations.InternalAggregations;
import org.opensearch.search.aggregations.support.MultiTermsValuesSourceConfig;

import java.util.HexFormat;
import java.util.List;
import java.util.Random;

/**
 * Writes the bytes OpenSearch 3.10.0 puts on the wire for forwarded {@code rare_terms} and {@code multi_terms}
 * aggregations (the builders, and the shard's {@code StringRareTerms}, {@code LongRareTerms} and
 * {@code InternalMultiTerms}), one {@code name hex} line each. Compiled as {@code CardRecordings.java} is, in the
 * aggregation's package for the package-private constructors:
 * {@code java org.opensearch.search.aggregations.bucket.terms.RareMultiRecordings > java-rare-multi-3.10.0.txt}.
 */
public final class RareMultiRecordings {
    static void line(String name, BytesStreamOutput out) {
        System.out.println(name + " " + HexFormat.of().formatHex(java.util.Arrays.copyOf(out.bytes().toBytesRef().bytes, out.size())));
    }
    static void answer(String name, InternalAggregation agg) throws Exception {
        BytesStreamOutput out = new BytesStreamOutput();
        InternalAggregations.from(List.of(agg)).writeTo(out);
        line(name, out);
    }
    static void request(String name, AggregationBuilder b) throws Exception {
        BytesStreamOutput out = new BytesStreamOutput();
        new AggregatorFactories.Builder().addAggregator(b).writeTo(out);
        line(name, out);
    }
    static MultiTermsValuesSourceConfig term(String field) {
        return new MultiTermsValuesSourceConfig.Builder().setFieldName(field).build();
    }
    static InternalMultiTerms.Bucket mb(String k, long n, long count, List<DocValueFormat> f) {
        return new InternalMultiTerms.Bucket(List.of(new BytesRef(k), n), count, InternalAggregations.EMPTY, false, 0, f);
    }
    static InternalMultiTerms.Bucket mbe(String k, long n, long count, long error, List<DocValueFormat> f) {
        return new InternalMultiTerms.Bucket(List.of(new BytesRef(k), n), count, InternalAggregations.EMPTY, true, error, f);
    }
    public static void main(String[] a) throws Exception {
        request("request.rare_terms", new RareTermsAggregationBuilder("r").field("tag"));
        RareTermsAggregationBuilder rare = new RareTermsAggregationBuilder("r").field("n").maxDocCount(5);
        rare.setPrecision(0.01);
        request("request.rare_terms_options", rare);
        request("request.multi_terms", new MultiTermsAggregationBuilder("m").terms(List.of(term("tag"), term("n"))));
        request("request.multi_terms_options", new MultiTermsAggregationBuilder("m").terms(List.of(term("tag"), term("n")))
            .size(3).shardSize(7).minDocCount(2).shardMinDocCount(1).order(BucketOrder.key(false)));
        request("request.multi_terms_show", new MultiTermsAggregationBuilder("m").terms(List.of(term("tag"), term("n")))
            .showTermDocCountError(true));

        BucketOrder order = BucketOrder.count(true);
        SetBackedScalingCuckooFilter none = new SetBackedScalingCuckooFilter(10000, new Random(0), 0.001);
        SetBackedScalingCuckooFilter keywords = new SetBackedScalingCuckooFilter(10000, new Random(0), 0.001);
        keywords.add(new BytesRef("x"));
        keywords.add(new BytesRef("y"));
        SetBackedScalingCuckooFilter numbers = new SetBackedScalingCuckooFilter(10000, new Random(0), 0.001);
        numbers.add(7L);
        numbers.add(-3L);
        SetBackedScalingCuckooFilter manyKeywords = new SetBackedScalingCuckooFilter(10000, new Random(0), 0.001);
        for (int i = 0; i < 30; i++) {
            manyKeywords.add(new BytesRef("k" + i));
        }
        SetBackedScalingCuckooFilter manyNumbers = new SetBackedScalingCuckooFilter(10000, new Random(0), 0.001);
        for (int i = 0; i < 25; i++) {
            manyNumbers.add((i - 5) * 1_000_003L);
        }
        answer("response.rare_keyword_many", new StringRareTerms("r", order, null, DocValueFormat.RAW, List.of(), 1, manyKeywords));
        answer("response.rare_long_many", new LongRareTerms("r", order, null, DocValueFormat.RAW, List.of(), 1, manyNumbers));
        answer("response.rare_keyword_empty", new StringRareTerms("r", order, null, DocValueFormat.RAW, List.of(), 1, none));
        answer("response.rare_keyword", new StringRareTerms("r", order, null, DocValueFormat.RAW, List.of(
            new StringRareTerms.Bucket(new BytesRef("a"), 1, InternalAggregations.EMPTY, DocValueFormat.RAW),
            new StringRareTerms.Bucket(new BytesRef("b"), 1, InternalAggregations.EMPTY, DocValueFormat.RAW)), 1, keywords));
        answer("response.rare_long", new LongRareTerms("r", order, null, DocValueFormat.RAW, List.of(
            new LongRareTerms.Bucket(1, 2, InternalAggregations.EMPTY, DocValueFormat.RAW),
            new LongRareTerms.Bucket(5, 1, InternalAggregations.EMPTY, DocValueFormat.RAW)), 2, numbers));

        List<DocValueFormat> f = List.of(DocValueFormat.RAW, DocValueFormat.RAW);
        BucketOrder co = BucketOrder.compound(BucketOrder.count(false));
        answer("response.multi_terms", new InternalMultiTerms("m", co, co, null, 12, false, 4, 0, f,
            List.of(mb("a", 1, 3, f), mb("b", 2, 1, f)), new TermsAggregator.BucketCountThresholds(1, 0, 10, 12)));
        answer("response.multi_terms_errors", new InternalMultiTerms("m", co, co, null, 12, true, 4, 3, f,
            List.of(mbe("a", 1, 3, 0, f), mbe("b", 2, 1, 7, f)), new TermsAggregator.BucketCountThresholds(1, 0, 10, 12)));
        answer("response.multi_terms_empty", new InternalMultiTerms("m", co, co, null, 12, false, 0, 0, f,
            List.of(), new TermsAggregator.BucketCountThresholds(1, 0, 10, 12)));
    }
}

package org.opensearch.search.aggregations.bucket.terms;
import java.util.*;
import org.opensearch.search.DocValueFormat;
import org.opensearch.search.aggregations.*;
import org.opensearch.search.aggregations.bucket.LocalBucketCountThresholds;
import org.opensearch.search.aggregations.pipeline.PipelineAggregator;
import org.opensearch.search.aggregations.InternalAggregation.ReduceContext;
/** Records the per-bucket doc_count_error_upper_bound InternalTerms.reduce leaves, at OpenSearch 3.10.0. */
public class ErrorRecordings {
  static LongTerms shard(String n, long err, int shardSize, long[][] kc, long bucketErr) {
    List<LongTerms.Bucket> bs = new ArrayList<>();
    for (long[] e : kc) bs.add(new LongTerms.Bucket(e[0], e[1], InternalAggregations.EMPTY, true, bucketErr, DocValueFormat.RAW));
    TermsAggregator.BucketCountThresholds th = new TermsAggregator.BucketCountThresholds(1, 0, 10, shardSize);
    return new LongTerms(n, BucketOrder.count(false), BucketOrder.count(false), null, DocValueFormat.RAW, shardSize, true, 0, bs, err, new TermsAggregator.BucketCountThresholds(1,0,10,shardSize));
  }
  static void reduce(long first, long second) {
    long[][] a = {{10, 5}, {20, 4}}, c = {{10, 3}, {30, 2}};
    ReduceContext rc = ReduceContext.forFinalReduction(null, null, s -> {}, PipelineAggregator.PipelineTree.EMPTY);
    LongTerms r = (LongTerms) shard("t", first, 2, a, 0).reduce(List.of(shard("t", first, 2, a, 0), shard("t", second, 2, c, 0)), rc);
    StringBuilder sb = new StringBuilder("shard errors " + first + "," + second + " bound=" + r.getDocCountError() + ":");
    for (LongTerms.Bucket b : r.getBuckets()) sb.append(" key" + b.getKey() + "=" + b.getDocCountError());
    System.out.println(sb);
  }
  public static void main(String[] args) {
    for (long[] e : new long[][] {{1, 2}, {2, 1}, {1, 1}, {3, 2}, {4, 4}}) reduce(e[0], e[1]);
  }
}

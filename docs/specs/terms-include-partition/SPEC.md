# Reader: a partition-based `include` of a keyword `terms`

## Serves

Roadmap R10.2, after [keyword-terms-agg](../keyword-terms-agg/SPEC.md) (AC-7), which refused `include: {"partition", "num_partitions"}`: I-13 (REST matches OpenSearch).

## Change

`include` accepts an object with `partition` and `num_partitions`, whole numbers, as `IncludeExclude.parseInclude` reads it. A keyword `terms` keeps the terms whose `floorMod(murmurhash3_x86_32(term bytes, 31), num_partitions)` equals `partition` (`PartitionedOrdinalsFilter`), through the same accepted-ordinals path as the regular-expression and set filters. The checks are OpenSearch's: a missing `partition` or `num_partitions`, a `partition` outside `[0, num_partitions)` ("Partition must be >=0 and < numPartition which is n"), an unknown key (`parse_exception`), and an `exclude` beside it ("Cannot specify any excludes when using a partition-based include", in either order).

Still refused as not supported yet: a partition-based `include` on a numeric field (the engine's numeric `terms` takes no filter), and a `partition` that is not a whole number.

## Acceptance criteria

- AC-1: the object parses with its checks, and on a real keyword index each partition holds exactly the terms the reference hash gives (computed with the standard Murmur3 x86 32-bit algorithm, seed 31), the partitions covering every term once.

## Test plan

`aggs/bucket/keyword_options_tests.rs` (parse and checks) and `keyword_partition_tests.rs` (the partitions of a real index).

## S3 request budget

Unchanged: the filter runs over the dictionaries the keyword `terms` already reads. LIST 0, HEAD 0, PUT 0.

## Switch

None: a request with this `include` was refused before.

## Risks

Not yet recorded against a stock node.

# The coordinator reduces each named metric aggregation across shards

## Serves

Parity plan R7.2, search fan-out and reduce, second piece: the aggregation half of the Java shell's `SearchMerge`, over the metric aggregations the reader already computes per shard (`oss_reader::aggs::metric::partial::reduce`). Bucket aggregations, `top_hits` across shards, the memory charge and the fan-out are their own pieces.

## Change

`oss-coordinator` gains `reduce::aggs`: `reduce_shards`, which takes what each answering shard computed (its aggregations as name and `Value`, in the order the shard listed them) and gives each name's reduced `Value`, names in the order they first appear, each reduced over the shards that have it in shard order.

## Acceptance criteria

- AC-1: a name every shard answered reduces to what `partial::reduce` gives for those values in shard order (a sum of `Sum`, the lowest of `Min`, and so on).
- AC-2: a name only some shards have reduces over those shards: a shard that lacks it adds nothing.
- AC-3: names come out in the order they first appear across the shards, once each.
- AC-4: values of one name that are not all of one kind are refused with `AggsError::Mismatched { name }`.
- AC-5: a `TopHits` value is refused with `AggsError::TopHits { name }`, because its hits need fetching and their shard known.
- AC-6: no shards, or shards with no aggregations, give no aggregations.

## Test plan

`crates/coordinator/tests/it/agg_reduce.rs`: one test per criterion over hand-built shard answers.

## S3 request budget

Zero: the reduce reads nothing.

## Bounds

One value per shard per name held at once; a shard sends one small value per aggregation, so the working set is the answers already in hand.

## Switch

None: the shell's reduce has no setting and no measured downside.

## Risks

The memory charge of the answers belongs to the piece that holds the node's budget.

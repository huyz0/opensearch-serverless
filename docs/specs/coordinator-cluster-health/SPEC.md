# The coordinator judges cluster health from what the shard heads say

## Serves

Parity plan R7.5, cluster and health REST, first piece: the decisions in the Java shell's `ClusterHealthHandler` (rest package, shell 429b4cbc) that need no node: which parameters are refused, how `wait_for_nodes` is read, how each shard and index is classified, how the cluster's status follows, and whether a wait is satisfied. The REST route, the polling loop, the lease and head reads and the rendering are the node assembly's and later pieces.

## Change

`oss-coordinator` gains `cluster::health`: `NodeCondition` (parse and `satisfied_by`), `refuse` (parameter refusals), `compute` (the `Health` from the node count and what each index's heads said) and `Health::satisfied`.

## Acceptance criteria

- AC-1: `NodeCondition::parse` reads `N`, `>=N`, `>N`, `<=N`, `<N` and `ge(N)`, `gt(N)`, `le(N)`, `lt(N)`, trimming spaces, and gives nothing for anything else; `satisfied_by` compares the node count with the operator.
- AC-2: `refuse` gives the shell's words, in this order, for: a `_cat` table option the cat table does not support (its own words); `wait_for_status` other than green or yellow; `wait_for_active_shards`; `wait_for_events`; a `level` other than cluster, indices or shards. Nothing for an accepted request.
- AC-3: when the node count cannot be read the status is red and nothing else is examined; an index that does not exist names itself as `missing` and ends the computation; an index whose descriptor cannot be read is red, and so is the cluster.
- AC-4: a shard whose head cannot be read is `unservable` (the index and cluster red); a shard with no head, no owner or an owner whose lease no longer holds it is `dormant`; one whose owner holds it is `owned`; dormant and owned shards are both active.
- AC-5: each index reports its shard count and its active, dormant and unservable counts, and (when asked) each shard's state; the cluster's counts are the indices' sums.
- AC-6: `satisfied` is true when an index was missing; false when a node condition fails; and with a wait for status (green or yellow) true only when the status is green; with no wait it is true.

## Test plan

`crates/coordinator/tests/it/cluster_health.rs`: one test per criterion; the refusal words asserted exactly.

## S3 request budget

Zero: the computation reads nothing; the heads and leases it is given are read once for each shard and owner by the caller.

## Bounds

One record for each shard asked of: the caller bounds the indices a request names.

## Switch

None: the shell's health has no setting and no measured downside.

## Risks

Yellow is never reported (there are no replicas), so a wait for yellow is the wait for green, as in the shell.

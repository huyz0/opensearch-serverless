# Bucket head release: only the last member open here gives the head back

## Serves

R11. Ported from the shell's `ShardHeadStore.releaseMember` and `BucketHeads.othersOpenOn` at huyz0/OpenSearch `429b4cbc`, on [bucket-heads](../bucket-heads/SPEC.md) and under [ADR 0026](../../adr/0026-bucket-manifests-in-the-writer.md). A bucket's members are owned through one head, so giving one member back must not give back the head while this node still holds another member open: the head is released only with the bucket's last open member here.

## Change

- `BucketHeads::others_open_on(bucket_id, node_id, except_index)`: whether `node_id` holds a member of the bucket other than `except_index` open. An implementation answers it from the node's own open shards.
- `ShardHeadStore::release` of a member: when another member is open on the node, it answers `true` with no request (the member is given back, the head stays). Otherwise one GET and, if the head names this node at the given term (and the process, when the head records one), one conditional PUT clearing the owner and its lease and keeping the term. The bucket head names the logs of every member, and one member published through all it holds says nothing of the others, so the releasing node is named among the head's writers; the next owner reads them.
- A head that moved (another owner, another term, another process) is left, and the answer is `false`; an absent head is `false`; a lost swap is `false`.
- Unbucketed shards release as before.

Not here: the descriptor-backed `BucketHeads`, renew of a member, the publish path's use of a bucket head, node wiring.

## Acceptance criteria

- AC-1: releasing a member while the node holds another open is `true` and makes no request.
- AC-2: releasing the last open member clears the bucket head's owner and lease, keeps its term, and names the releasing node among the writers; 1 GET and 1 PUT.
- AC-3: after that, a member taken by another node takes the head at term + 1 naming the releasing node among its writers; `taken_from` is none, since the head had no owner.
- AC-4: a release at another term, by another node, or of another process is `false` and writes nothing; an absent head is `false` with one GET.
- AC-5: an unbucketed shard of the same store releases as before.

## Tests

`crates/cluster/tests/leases/bucket_heads.rs`, with `others_open_on` answered from a test map.

## S3 request budget

A member release: 0 requests while another member is open here, otherwise 1 GET and 1 conditional PUT (2 for an unbucketed shard as well). 0 LIST.

## Risks

- The releaser is named a writer of the bucket head, unlike an unbucketed release, so its own next take of the bucket reads its own log; the Java does the same for the reason above, and it costs one replay read at most.

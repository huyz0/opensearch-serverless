# Bucket policy: whether an index is grouped, and with what

## Serves

R11. Ported from the shell's `BucketPolicy` (`AUTO`, `OFF`, `parse`, `check`, `wanted`, `group`) and the `INDEX_POLICY`, `CLUSTER_POLICY` and `GROUP` settings of `BucketSettings` at huyz0/OpenSearch `429b4cbc`, on [bucket-settings](../bucket-settings/SPEC.md) and under [ADR 0026](../../adr/0026-bucket-manifests-in-the-writer.md). One setting says whether an index is placed in a bucket, per index or for the cluster, the index's winning; unset everywhere, the node's `serverless.buckets.enabled` decides. A second names the group an index is bucketed with, the index name by default. The policy only decides placement: which head owns an index follows its descriptor on every node, so changing it never splits ownership.

## Change

- `oss_writer::bucket::policy`: `INDEX_POLICY` (`index.serverless.bucket`, index scope), `CLUSTER_POLICY` (`serverless.bucket`, node scope), `GROUP` (`index.serverless.bucket_group`, index scope), `SETTINGS`.
- `check(value)`: `auto` and `off`, in any letter case, and the empty value (unset) are accepted; anything else is refused.
- `wanted(layers)`: the index's value, else the cluster's, else the node's `serverless.buckets.enabled`; `auto` is true and `off` false. An empty index or cluster value is unset at its layer, and one that does not parse is skipped, so a bad cluster value cannot stop an index or a node from deciding. A node value that is no boolean is refused.
- `group(index_name, index)`: the index's group setting, or its name when that is unset or empty.
- `crates/node/src/settings.rs` registers `SETTINGS`.

Not here: heads and placement, the descriptor's bucket field, node wiring. Nothing in a node calls this yet.

## Acceptance criteria

- AC-1: with nothing set anywhere, `wanted` is the node's `serverless.buckets.enabled`, false by default and true when the node sets it.
- AC-2: the index's `auto` or `off` wins over the cluster's, and the cluster's over the node's; letter case is ignored.
- AC-3: an empty value is unset at its layer; an index or cluster value that is neither `auto` nor `off` is skipped to the next layer.
- AC-4: a node `serverless.buckets.enabled` that is no boolean is refused with a `SettingError`.
- AC-5: `check` accepts `auto`, `off`, `AUTO` and the empty value, and refuses `on`, `true` and ` auto`.
- AC-6: `group` is the index's `index.serverless.bucket_group`, and the index name when it is unset or empty.
- AC-7: the three settings, with those of `settings`, form a valid registry.

## Tests

`crates/writer/tests/bucket_policy.rs`; the node's registry test counts the settings.

## S3 request budget

None: pure functions over settings, no blob request, no timer.

## Risks

- A bad cluster value is skipped silently, as the Java does, so a typo reads as the node default; `check` is for the settings API to refuse it up front.

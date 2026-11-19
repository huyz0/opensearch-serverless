# The node decides each open's prefetch: a setting and a memory charge

## Serves

The two review findings on the read prefetch: its buffers (up to 8 MiB for one open) were not charged to the `open_indexes` pool, and prefetch was on for every open with no setting to turn it off.

## Change

- `oss-format`: an index descriptor carries its `settings` object's bytes.
- `oss-reader`: `IndexMeta.settings` holds the index's settings, and `prefetch::planned_bytes` gives what a plan reads.
- `oss-node`: `serverless.reader.open_prefetch.enabled` (default true), resolved for each open as the index's `index.serverless.reader.open_prefetch.enabled`, else the cluster register's, else the node's, else the default; an open that prefetches holds the planned bytes from `open_indexes` until the open returns.

## Acceptance criteria

- AC-1: a descriptor's `settings` object is kept as its bytes (`None` when the descriptor has none) and a settings field that is not an object is refused as corrupt.
- AC-2: `IndexMeta.settings` reads those bytes as flat settings, nested keys joined by `.`; a descriptor with `{}` or none reads as no settings.
- AC-3: `planned_bytes` is the sum of the plan's ranges' lengths, zero for no ranges.
- AC-4: the setting resolves index, then cluster, then node, then true; an index value of `false` opens that index without prefetch whatever the cluster says, and a cluster value of `false` turns it off for every index that sets nothing.
- AC-5: a prefetching open charges the planned bytes to `open_indexes` while it runs and gives them back when the open returns, so a pool with no room for them opens the shard without prefetch, not with a failure; the charge never stays with the open shard.
- AC-6: an index whose setting is off, or whose node or cluster turns it off, is opened with prefetch off and holds nothing from the pool for it.

## Test plan

`crates/format/tests/shell_blobs.rs` (AC-1), `crates/reader/src/prefetch/plan_tests.rs` (AC-3), `crates/node/tests/it/open_prefetch.rs` (AC-2, AC-4 to AC-6) over the compat index through the emulator, reading the `open_indexes` pool's use.

## S3 request budget

Unchanged by this change: the prefetch's round is as it was, and an open that falls back or is switched off reads file by file as before.

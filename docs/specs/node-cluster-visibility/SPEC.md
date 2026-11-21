# The refresh visibility is a cluster setting a running node follows

## Serves

Parity plan R9.3: `serverless.search.refresh_visibility` was read once, from `OSS_REFRESH_VISIBILITY`, when a node booted, so changing it meant restarting every node. The Java shell reads it from the cluster settings. Builds on [node-boot-tail](../node-boot-tail/SPEC.md), [node-refresh-wait](../node-refresh-wait/SPEC.md) and [reader-tail-fresh](../reader-tail-fresh/SPEC.md); the register is [cluster-config](../cluster-config/SPEC.md).

## Change

- `serverless.search.refresh_visibility` is a dynamic setting: a value in the cluster register (`PUT /_cluster/settings`) overrides the node's (`OSS_REFRESH_VISIBILITY`), and removing it returns to the node's. A value that is not one of `owner`, `serving`, `everywhere` is ignored (the node keeps the one in force; the update route refuses it before it is stored).
- A node with the tail overlay on reads the register on a thread of its own, every `OSS_CLUSTER_SETTINGS_POLL_MS` (default 30,000), puts the snapshot in force (`oss_types::settings::Live`) and applies the mode it names to the writer role (its refresh wait) and the reader's freshness check (its grants) without a restart. A node with the overlay off starts no thread and makes no request: nothing it runs reads a dynamic setting.
- `oss_node::cluster_live::ClusterFeed` is the read: one register GET into a `Live`, the node's code taking the snapshot from it.

## Acceptance criteria

- AC-1: reading the register puts its values in force, newer generations only; a register never written leaves nothing in force; a removed key goes. (A list-valued key is not a value and is skipped.)
- AC-2: the mode in force is the cluster's value when it is a valid choice, else the node's.
- AC-3: `TailFresh` changes between `owner`, `serving` and `everywhere` while running: grants and the apply of held records follow the mode set last.
- AC-4: a mode set on the node's handle reaches the writer role's refresh visibility and the reader's check together.
- AC-5: `OSS_CLUSTER_SETTINGS_POLL_MS` is a whole number of milliseconds above zero, refused by name otherwise; default 30,000.
- AC-6: in a booted node with the overlay on, a `PUT /_cluster/settings` changing the setting reaches that node's writer and reader checks within two polls, and removing it goes back to the node's value.
- AC-7: a booted node with the overlay off makes no cluster settings request.

## Test plan

AC-1, AC-2: unit tests in `crates/node/src/cluster_live.rs`. AC-3, AC-4: `crates/node/tests/tail_fresh.rs`. AC-5: `crates/node/src/boot/config.rs`. AC-6, AC-7: `crates/node/tests/boot.rs` over a file store with a short poll.

## S3 request budget

With the overlay off, no change (GET 0 added). With it on, one GET of the cluster settings register a poll a node (120 an hour at the default; about 1,000 nodes x 120 = 120,000 GETs an hour cluster-wide, a few cents), independent of data and of indexes, no LIST, no PUT. It is a per-node cost like the lease read, at a third of the lease read's rate. Not a conditional read: the object store here offers none on a register.

## Switch

The poll runs only with `serverless.search.tail_overlay.enabled`, which is off by default, so the default adds no request. The visibility default stays `everywhere` once the overlay is on: measured on the freshness meter (visible p99 about 400 ms against 497 ms under `owner`, no misses, about 1.2 fewer PUTs and 0.9 fewer GETs a refresh write) with a cost of 5 to 8 ms on a search that finds no grant; the overlay itself stays off by default until the takeover-with-overlay fault suites exist.

## Risks

- A mode change on a live shard: going to `everywhere` makes readers ask writers for grants and the writer begin to keep records; going away from it stops the asks and the keep lapses. Each node changes on its own poll, so for up to two polls a cluster mixes modes. Mixed modes are safe: a reader asking for a grant of a writer that has not begun keeping is answered as it is for a first search today; a writer that waits for readers that no longer check only waits.

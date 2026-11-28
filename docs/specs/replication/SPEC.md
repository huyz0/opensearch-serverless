# Cross-cluster replication, slice 1: remotes and a read-only store reader

## Serves

[ADR 0085](../../adr/0085-cross-cluster-replication-copies-published-segments-between-object-stores.md), slice 1 of 6. The owner's rule (2026-11-28): native, no Java in the runtime path. Nothing here copies a byte or writes to a leader; it makes a leader's store readable and proves what can be read.

## Change

A **remote** is a store location. `PUT _cluster/settings` accepts `cluster.remote.<alias>.store`, a document with `type` (`s3` or `fs`), `bucket` and `prefix` (s3) or `root` (fs), and an optional `profile` naming read credentials the node already holds. The setting is persisted by the cluster-settings document that exists today; no new document set and no new blob directory.

1. **The allow-list.** `OSS_REPLICATION_REMOTE_ROOTS` (empty by default, so nothing is readable) lists the `bucket[/prefix]` and directory roots a remote may sit under. The checks are the stock-repository validators of `OSS_STOCK_REPO_ROOTS` and `OSS_STOCK_REPO_S3_ROOTS`, reused rather than copied (extracted to one shared function in the slice's first commit, with the stock restore's tests unchanged): canonical path, no symlink, and no overlap in either direction with this deployment's own store. A store outside the list, or overlapping, is 400 `illegal_argument_exception` naming the rule.
2. **A read-only remote handle.** `oss-replication::remote::RemoteStore` wraps an `oss-blob` store opened from the setting. It exposes only `get` and `head` through a trait with no write method, so a write to a leader is a compile error, not a runtime check. It is metered like every store: its requests are the follower tenant's.
3. **Reading a leader.** `RemoteStore::shard_manifest(index, uuid, shard)` reads the leader shard's commit register (the key of `ShardRef::manifest_key`) and returns its generation and the files it names; `index_descriptor(index)` reads the leader's descriptor (`descriptor_key`). A missing leader index is a typed `RemoteMissing`, not a panic or a generic 500. The reads hold no lock and no lease and never list.
4. **A way to see it works.** `GET /_remote/<alias>/_info` is not a stock route and is not added. The slice's evidence is `RemoteStore` unit tests and the settings path through `PUT/GET _cluster/settings`: the stock `PUT _cluster/settings` body with `seeds` is accepted, the seeds are ignored, and the response carries the warning ADR 0085 names.

Not here: copying, a follower index, any `/_plugins/_replication` route, the poll loop, metadata following, auto-follow. `serverless.replication.enabled` is registered by this slice (default off, nothing reads it yet) so slice 2 adds no setting churn.

## Acceptance criteria

- AC-1: a remote naming a store under an allowed root is accepted and read back by `GET _cluster/settings`; one outside every root, one that overlaps this deployment's store in either direction, and one whose path is a symlink are each 400 naming the rule. With `OSS_REPLICATION_REMOTE_ROOTS` empty every remote is refused.
- AC-2: `RemoteStore` has no method that writes, deletes or lists; a compile-fail test (`trybuild`-style, or a doc test marked `compile_fail`) holds that a `put` call does not compile. The wrapped store is opened read-only: against the fs store and the emulator a write through the handle's inner store is not reachable from the crate's public API.
- AC-3: `shard_manifest` returns the generation and file names of a leader shard written by the writer crate in the same test (not a hand-built blob), for a shard with several generations; `index_descriptor` returns the leader's shard count and mappings; a missing index and a missing shard are `RemoteMissing`.
- AC-4: reading a leader shard manifest makes exactly 1 GET (1 HEAD when the generation is asked for alone); reading a descriptor makes 1 GET; neither lists, asserted by the request meter. An idle node with remotes configured makes zero requests.
- AC-5: `cluster.remote.<alias>.seeds` is accepted and ignored with a warning in the response; a `store` body with an unknown `type`, a missing `bucket`, an alias that is not a valid name, or more than 16 remotes is 400.
- AC-6: with `serverless.replication.enabled` off (the default) the remote settings still validate and persist, and nothing else changes: the existing suite passes unchanged, and both settings values run in CI.

## Test plan

AC-1 and AC-5: HTTP tests through the real settings route in `crates/node/tests/server/replication_remotes.rs`, red first by the current behaviour (an unknown setting key is refused, so the accepted-remote test fails before the code). AC-2: the compile-fail test, red first by a handle that exposes `put`. AC-3: a writer-built leader in an in-process store, red first by a reader that returns the descriptor's generation instead of the manifest's. AC-4: the meter. AC-6: the existing settings and config tests unchanged, plus one for the default.

## S3 request budget

Reading a leader shard manifest is 1 GET, a descriptor 1 GET; there is no standing cost and no poll in this slice. Rows go in [request-budgets](../../request-budgets.md).

## Switch

`serverless.replication.enabled` (`OSS_REPLICATION`), default **off**, per ADR 0085: the poll cost of later slices is an operator's choice. This slice adds no cost, so its remote setting and reader are always available; the switch gates only the routes and the poll that later slices add. The measurement that decides the default is slice 6's.

## Risks

- The allow-list check is a security boundary: a remote must not let a request make the node read an arbitrary bucket. The stock validators are the tested code to reuse; extracting them must not weaken them, so their existing tests run unchanged and the planted-violation rule applies to the new check.
- A leader written by a different build may carry a manifest this reader does not know: `shard_manifest` returns the format error typed, and slice 2 decides what to do with it.
- Credentials: `profile` names credentials the node already holds; it never accepts a secret in the request. The profile mechanism itself may not exist for s3 yet; if not, slice 1 uses the node's own credentials only and says so in the setting's documentation.

# Node roles

## Serves

R12.3, index and search roles. Ported from the shell's `serverless.roles` (`ingest`, `search`; default both; empty is coordinating only) at huyz0/OpenSearch, with the placement that already reads the roles a lease names ([placement](../placement/SPEC.md), [node-cluster-routing](../node-cluster-routing/SPEC.md)).

## Change

- `OSS_ROLES` (node scope, read at boot): a comma list of `ingest` and `search`; unset or empty is both, as today; `none` alone is a coordinating-only node (no role). An unknown role, or `none` beside another, is refused. Repeats count once.
- The node's lease names exactly those roles. Placement already picks writers among leases with `ingest` and tail readers among leases with `search`, so a node without a role is never chosen for it.
- A node without `ingest` opens no shard: its writer role is built with no opener and proactive takeover is off. A write it receives (its own REST request, or a stale forward from a peer) goes to the rendezvous owner among the live `ingest` leases; with none live the item fails 503 `unavailable_shards_exception`, as a gone owner does. It never opens the shard itself.
- A node without `search` is not named as a tail reader, so writers push nothing to it. It still answers a search sent to it, from published segments; which traffic reaches it is the ingress rule in [docs/deployment.md](../../deployment.md).
- `docs/deployment.md` states the two-pool layout: write paths to the ingest pool, the rest to the search pool, and what each role does and does not do.

## Acceptance criteria

- AC-1: with `OSS_ROLES` unset or empty the node's roles are `ingest` and `search`; `search`, `ingest` and `ingest,search` in any order and with spaces are accepted; `none` is no role; `bogus`, `ingest,bogus` and `none,search` are refused with the variable named.
- AC-2: a booted node's lease names the configured roles (read back through the membership).
- AC-3: with an `ingest` node and a `search`-only node in one cluster, a bulk sent to the search-only node is applied by the ingest node (the document is found, and the ingest node holds the shard); the search-only node holds no shard.
- AC-4: a search-only node alone (no live `ingest` lease) answers a write 503 and holds no shard.
- AC-5: a search-only node answers a search for a document the ingest node published.
- AC-6: the lease of a node without `search` is not chosen as a tail reader (the existing `readers_for` over the booted leases).

## Test plan

`crates/node/src/boot/config/tests.rs` is full; the parse tests are in `crates/node/tests/server/boot_roles.rs` (AC-1, AC-2) with the cluster tests (AC-3 to AC-6), each seen failing with the behaviour removed.

## S3 request budget

Unchanged: a node without `ingest` makes fewer requests (no takeover, no log, no heads).

## Switch

`OSS_ROLES`, default both roles: no behaviour changes unless it is set.

# A node that starts from configuration

## Serves

Parity plan R7.6, node `main`: until now `main` printed the roles. A node with the reader and writer roles now starts from environment configuration: it opens its store, enrols its lease and keeps it renewed, builds the writer's committer over its own shard heads, assembles the roles, registers the health routes and listens. Invariants: S3 requests are the bill (the lease renewal is the only periodic request, one swap and one refresh a tick, as [node leases](../node-leases/SPEC.md) priced it); fencing is by conditional create, never a lock (the lease is liveness, the shard head is the fence); bounded memory (the budget splits the node's bytes into pools).

## Change

`oss-node`, with the `reader` and `writer` features, gains `boot`:

- `NodeConfig::from_lookup` reads `OSS_NODE_ID` (required), `OSS_STORE` (`fs` with `OSS_FS_ROOT`, or `s3` with `OSS_S3_ENDPOINT`, `OSS_S3_BUCKET`, `OSS_S3_REGION`, `OSS_S3_ACCESS_KEY`, `OSS_S3_SECRET_KEY`), `OSS_CLUSTER`, `OSS_BASE`, `OSS_HTTP_ADDR`, `OSS_TRANSPORT_ADDR` (the native transport listener, default `127.0.0.1:9300`), `OSS_ADVERTISE_ADDR` (the transport address the lease carries; the bound transport address when unset), `OSS_MEMORY_BYTES`, `OSS_LEASE_TTL_MS`, `OSS_LEASE_REFRESH_MS` (how often the fleet's leases are read again, default a third of the lease lifetime; [node-lease-refresh](../node-lease-refresh/SPEC.md)), `OSS_JANITOR_EVERY_MS` (how often the janitor sweeps and the dead-log pass runs, default 60,000), `OSS_TOMBSTONE_SWEEP_MS` (how often the one sweeping node removes expired tombstones, default 3,600,000, 0 off; [node-tombstone-sweep](../node-tombstone-sweep/SPEC.md)) and `OSS_INDEX_CONFIRM_MS` (how long a writer trusts an index it read before confirming it by the change stripes, default 5,000, 0 off; [node-index-cache](../node-index-cache/SPEC.md)), and refuses what it cannot use with a message that names the variable. A secret never appears in `Debug`.
- `start` builds the node: the store, the memory budget split into pools by fixed shares, a `Membership` whose lease (roles `ingest` and `search`) is enrolled before the node listens, the write log's committer over `HeadAdmission`, a `HeadOpener` so a write opens the shard it names, `DescriptorRoutes` for indexes, the native transport server with the owner-side handler for a forwarded bulk, `ClusterRoutes` as the writer's routes (owner by placement over the live members, forward and remote refresh over the transport), `assemble`, `StoreHealth` through `Assembled::with_health`, and the listener.
- A renewal thread renews the lease and refreshes the membership every `lease_ttl / 3`; dropping the node stops it and releases the lease.
- `main` reads the process environment, starts the node and parks.
- The transport server answers `BULK_ACTION` with `BulkHandler` ([node-bulk-handler](../node-bulk-handler/SPEC.md)), signed with the cluster's transport secrets; it refuses with a rejection until the node has assembled.
- Not here, listed: forwarding of reads and of `_refresh` by REST (a read is served by the node that holds the shard); takeover of a dead node's log; the tail overlay; signal handling (a killed node's lease lapses in one lifetime).

## Acceptance criteria

- AC-1: `from_lookup` fills every default, and refuses a missing node id, an unknown store type, a missing variable of the chosen store, an address or number that does not parse, a base that does not end in `/`, and a memory total too small for the pools.
- AC-2: the configuration's `Debug` does not contain the S3 secret.
- AC-3: a node started on a file store answers `GET /_cluster/health` with one node, and its lease register exists in the store before it listens.
- AC-4: through that node, `PUT /{index}`, `PUT /{index}/_doc/{id}`, a refresh and `GET /{index}/_doc/{id}` write and read a document: the shard opened through the head opener.
- AC-5: a second node on the same store with another id is seen by the first after a renewal tick (`number_of_nodes` 2).
- AC-6: dropping a node stops its renewal and releases its lease: the members index lists one member at once, and still one after a renewal tick.
- AC-8: two nodes on one store: a document written through the node that does not own its shard (with `refresh=true`) is applied and refreshed by the owner, and each document is then readable from exactly one of the two nodes, both holding some.
- AC-9: `Node::crash` stops the node without releasing its lease: the members index still lists it at once, as after a killed process.
- AC-7: a node started again on the same store and id reads the epochs its register names before it enrols, keeps them in its lease, and writes in an epoch above them, so a successor still fences what the earlier process wrote.

## Test plan

`crates/node/src/boot/config.rs` unit tests for AC-1 and AC-2; `crates/node/tests/server/boot.rs` over a temp-dir file store and real sockets for AC-3 to AC-8.

## S3 request budget

Start: one read of the node's own lease register, then its enrolment. Steady: one lease renewal and one membership refresh every `lease_ttl / 3` per node, at the per-node cost the `lease.*` rows of the request budgets price ([node leases](../node-leases/SPEC.md)); no request scales with indexes. A failed tick is retried by the next.

## Risks

- A read sent to a node that does not hold the shard answers 503 naming the holder; the client must route reads, or a later piece forwards them.
- Ownership follows the live members each node sees; during a membership change two nodes may briefly both take writes for one shard, and the shard head's conditional write makes one of them fail.

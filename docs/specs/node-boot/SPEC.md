# A node that starts from configuration

## Serves

Parity plan R7.6, node `main`: until now `main` printed the roles. A node with the reader and writer roles now starts from environment configuration: it opens its store, enrols its lease and keeps it renewed, builds the writer's committer over its own shard heads, assembles the roles, registers the health routes and listens. Invariants: S3 requests are the bill (the lease renewal is the only periodic request, one swap and one refresh a tick, as [node leases](../node-leases/SPEC.md) priced it); fencing is by conditional create, never a lock (the lease is liveness, the shard head is the fence); bounded memory (the budget splits the node's bytes into pools).

## Change

`oss-node`, with the `reader` and `writer` features, gains `boot`:

- `NodeConfig::from_lookup` reads `OSS_NODE_ID` (required), `OSS_STORE` (`fs` with `OSS_FS_ROOT`, or `s3` with `OSS_S3_ENDPOINT`, `OSS_S3_BUCKET`, `OSS_S3_REGION`, `OSS_S3_ACCESS_KEY`, `OSS_S3_SECRET_KEY`), `OSS_CLUSTER`, `OSS_BASE`, `OSS_HTTP_ADDR`, `OSS_ADVERTISE_ADDR` (the transport address the lease carries), `OSS_MEMORY_BYTES` and `OSS_LEASE_TTL_MS`, and refuses what it cannot use with a message that names the variable. A secret never appears in `Debug`.
- `start` builds the node: the store, the memory budget split into pools by fixed shares, a `Membership` whose lease (roles `ingest` and `search`) is enrolled before the node listens, the write log's committer over `HeadAdmission`, a `HeadOpener` so a write opens the shard it names, `DescriptorRoutes` for indexes, `assemble`, `StoreHealth` through `Assembled::with_health`, and the listener.
- A renewal thread renews the lease and refreshes the membership every `lease_ttl / 3`; dropping the node stops it and releases the lease.
- `main` reads the process environment, starts the node and parks.
- Not here, listed: the transport server and the owner's handler for a forwarded bulk, so every shard is written by the node that receives the write (`DescriptorRoutes` answers `Local`) and `ClusterRoutes` is wired when an owner can answer; takeover of a dead node's log; the tail overlay; signal handling (a killed node's lease lapses in one lifetime).

## Acceptance criteria

- AC-1: `from_lookup` fills every default, and refuses a missing node id, an unknown store type, a missing variable of the chosen store, an address or number that does not parse, a base that does not end in `/`, and a memory total too small for the pools.
- AC-2: the configuration's `Debug` does not contain the S3 secret.
- AC-3: a node started on a file store answers `GET /_cluster/health` with one node, and its lease register exists in the store before it listens.
- AC-4: through that node, `PUT /{index}`, `PUT /{index}/_doc/{id}`, a refresh and `GET /{index}/_doc/{id}` write and read a document: the shard opened through the head opener.
- AC-5: a second node on the same store with another id is seen by the first after a renewal tick (`number_of_nodes` 2).
- AC-6: dropping a node stops its renewal and releases its lease: the other node sees one member again.

## Test plan

`crates/node/src/boot/config.rs` unit tests for AC-1 and AC-2; `crates/node/tests/boot.rs` over a temp-dir file store and real sockets for AC-3 to AC-6.

## S3 request budget

Start: one membership read, one lease create, one head-opener read a write. Steady: one lease swap and one membership refresh every `lease_ttl / 3` per node; an idle index makes none.

## Risks

- Without the transport server a multi-node cluster does not steer writes: two nodes may both take writes for one shard, and the shard head's conditional write makes one of them fail. Single-node use is complete.

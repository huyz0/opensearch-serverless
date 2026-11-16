# oss-cluster

Fleet membership: which nodes are alive and what they accept, from one lease register a node, renewed by compare-and-swap, as the Java shell's `membership` package does, so Rust and Java nodes on one bucket see each other.

## Owns
A node's lease (renew, revoke, release, self-validity with the clock-skew margin), the members index that turns a refresh into a read, the lease digest one node writes so the rest read one register instead of every lease, pruning of leases dead for ten lifetimes, membership deltas, and the liveness oracle a takeover asks. Placement: the owner of a shard's writes (rendezvous hashing), the readers of its searches with the hot-shard spread and zone preference, the takeover winner and hand-off targets, each computed alike by every node from the leases with the Java's hashes. Routing: which physical shard a document lives in, by the unit layout of an index created with a shard mode (the 64-bit routing hash, splits and merges of units) or by core's murmur3 modulo the shard count for one without, as the Java computes it. Early revocation of a refused peer's lease, off by default. The transport secret: the register's keys, minted by a deployment's first node and cached for one lease TTL, the source of the transport's request MAC.

## Depends on
`oss-blob` (registers), `oss-format` (the lease, index, digest and secret bytes), `oss-transport` (the MAC's `SecretSource`), `getrandom` (a minted key), `oss-types` (the early-revoke setting). Never a role crate.

## Entry points
`src/lib.rs`: `Membership` and `LivenessOracle` over any `BlobStore`, with a `LeaseClock` the tests drive; `placement::{owner, readers_for, FleetView, ZonePreference, HotShardSpread}`, pure over the leases; `routing::{UnitLayout, DocumentRouting, routing_hash}`, pure over the descriptor's layout; `transport_secret::{TransportSecretStore, CachedTransportSecrets}`; `EarlyRevoke` with a `ConnectProbe` (`TcpProbe`). Tested on the emulator with a simulated clock and exact request counts (`tests/`). Specs: [docs/specs/node-leases](../../docs/specs/node-leases/SPEC.md), [docs/specs/placement](../../docs/specs/placement/SPEC.md), [docs/specs/routing](../../docs/specs/routing/SPEC.md); decision: [ADR 0015](../../docs/adr/0015-cluster-crate-and-node-leases.md).

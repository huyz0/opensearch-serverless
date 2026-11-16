# oss-cluster

Fleet membership: which nodes are alive and what they accept, from one lease register a node, renewed by compare-and-swap, as the Java shell's `membership` package does, so Rust and Java nodes on one bucket see each other.

## Owns
A node's lease (renew, revoke, release, self-validity with the clock-skew margin), the members index that turns a refresh into a read, the lease digest one node writes so the rest read one register instead of every lease, pruning of leases dead for ten lifetimes, membership deltas, and the liveness oracle a takeover asks. Placement and routing (R4.2, R4.3) come next.

## Depends on
`oss-blob` (registers), `oss-format` (the lease, index and digest bytes). Never a role crate.

## Entry points
`src/lib.rs`: `Membership` and `LivenessOracle` over any `BlobStore`, with a `LeaseClock` the tests drive. Tested on the emulator with a simulated clock and exact request counts (`tests/`). Spec: [docs/specs/node-leases](../../docs/specs/node-leases/SPEC.md); decision: [ADR 0015](../../docs/adr/0015-cluster-crate-and-node-leases.md).

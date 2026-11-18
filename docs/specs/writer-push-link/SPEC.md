# Writer push link: placement targets and the signed transport call

## Serves

Parity plan R6.5 (the reader takes pushes), fifth slice, and [ADR 0016](../../adr/0016-native-transport-and-early-revoke.md). Gives the [writer push sender](../writer-push-sender/SPEC.md) its two traits.

## Change

`oss-node` gains `push_link` (`crates/node/src/push_link.rs`, `writer` feature, which now also brings `oss-cluster` and `oss-format`): `PlacementTargets` ranks the readers of a shard from the leases the node holds (`readers_for` with the search role, at least two, a hot shard's spread, never this node), and `TransportPushLink` sends a push to one of them through the transport client, signed with the request MAC, waiting at most 500 ms, and gives the reader's open flag. Both read the lease snapshot already in memory, so neither makes a store request.

Not changed, by design and listed: wiring the sender into the writer's publish (the node's writer role does not exist yet), compressing a push to another zone, the blocks a push may carry, the writer answering newest-commit, the reader asking it, the tail overlay.

## Acceptance criteria

- AC-1: a push sent through the link reaches a reader's guarded handler with every field as sent, verifies, and the reader's open answer comes back.
- AC-2: a push the reader refuses (signed with another secret), to a node the leases do not name, to a revoked lease, to an address that is none, or to one nobody listens on is a link error, and the reader takes nothing.
- AC-3: the targets are the ranked search nodes in rank order other than this node, at least two ranked before this node is left out, a wider spread when asked, never a node without the search role; no leases, no targets.

## Test plan

`crates/node/tests/push_link.rs`: a real server on loopback with the guarded handler, and the placement ranking compared with `readers_for`.

## S3 request budget

None: leases are the snapshot the node holds, and a push goes between nodes.

## Switch

None: the link is inert until the writer role calls the sender, which has its own switch.

## Risks

- A peer's lease address is a hint; a wrong one is a lost push and the reader's open reads the register.

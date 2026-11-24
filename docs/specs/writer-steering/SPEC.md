# A write for a shard this node does not hold goes to a member with room when this node is nearly full

## Serves

R13.2: the shell's `writerFor` steering. [writer-hand-off](../writer-hand-off/SPEC.md) lets a full node give idle shards up; without steering a write that reaches it reopens the shard it just gave up. This sends that write to the member the fleet view picks instead.

## Change

- `WriterRole::hand_off` keeps the members, the lease clock reading, the lease lifetime and the node id it was last called with (a copy a refresh).
- `WriterRole::owner`, for a shard not held here, not redirected, and that the routes would open here: with `serverless.activation.hand_off.enabled` on, a cap set, this node nearly full (`FleetView::nearly_full`) and a live writer with room (`member_with_room`), the owner is `FleetView::writer_for` (rendezvous over live writers with room, this node aside), reached by the existing forward path. Otherwise the owner is what the routes say, as before.
- A steered shard is remembered with its member for 10 s (at most 4,096 shards, a full memory cleared), and `owner` answers the same member meanwhile.
- `ShardCapStats::steered` counts writes sent on.
- No new setting: steering rides `hand_off.enabled`, default false.

## Acceptance criteria

- AC-1: with hand-off on, this node nearly full and a member with room, `owner` of an unheld shard is that member, and `steered` is 1.
- AC-2: a shard held here is `Local`, whatever the fleet says.
- AC-3: with the setting off, this node not nearly full, or no member with room, an unheld shard is not steered and `steered` stays 0.
- AC-4: a shard steered to a member keeps going to that member for 10 s whatever the fleet then says, so the refresh that follows a write goes where the write went (a refresh sent elsewhere answered "not open on this node" in a three-node run).
- AC-5: a shard the routes give another node is not steered: the routes' owner stands, so a shard another node holds is never sent to a third.

## Test plan

`crates/node/tests/writer_hand_off.rs`, over a role on the emulator with hand-made leases.

## S3 request budget

None: the decision reads the copy a refresh kept, no store request. A steered write costs the forward the routes already make for any non-local owner.

## Switch

`serverless.activation.hand_off.enabled`, default false; flips with hand-off.

## Risks

- The copy is up to one refresh old: a write may go to a member that has since filled; that member then refuses or makes room as for any write.
- A forwarded write is one more hop; only a node nearly full of its cap pays it.

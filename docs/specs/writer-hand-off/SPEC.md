# A writer node nearly at its cap, or far above the fleet's mean, hands idle shards to a member with room

## Serves

R13.2: the shell's `handOffToMemberWithRoom` ([writer-shard-cap](../writer-shard-cap/SPEC.md) is the cap it acts on). A node that joins, or restarts empty, takes only what happens to reach it while the full nodes stay full until their shards go idle; handing idle shards off lets the next write for one be placed where there is room. The pure decision (`FleetView`: `nearly_full`, `member_with_room`, `above_fleet_share`, `member_below_mean`) is already in `oss_cluster::placement`; this wires it.

## Change

- The node's lease reports `held` (shards held) and `cap` (`serverless.activation.max_shards`, 0 for none) on every renewal, which is the same PUT as before. Peers read them; `in_use` and `refused` stay -1.
- `WriterRole::hand_off(members, now, ttl, local)` builds a `FleetView` and, when it should hand off, lets go of idle shards by the cap's eviction (publish if unpublished, close, give the head back):
  - nearly full (at least 90% of the cap) while a live writer holds at most 75% of its cap: down to 80% of the cap;
  - else far above the mean (above 1.5 times it, and holding at least 25% of the cap) while a live writer holds fewer than the mean: down to 1.25 times the mean;
  - at most a tenth of the cap, at least one, a pass; only shards unused for `evict_after_millis`.
- The renewal thread runs it after each fleet refresh.
- `serverless.activation.hand_off.enabled` (node scope, `OSS_HAND_OFF`), **default false**: with no cap there is nothing to be nearly full of, and the Java measured that shedding on a timer when no member had room made nodes give shards up and take them back. A node with it off makes no pass, whatever the leases say.

Not in this change: steering a write for an unheld shard to the member with room (a later piece; until then the write reaches this node's open path as before), the fleet size from `in_use` and `refused`, a stats endpoint.

## Acceptance criteria

- AC-1: nearly full, with a live writer at or below 75% of its cap, a pass lets go of one idle shard (a tenth of a cap of four, at least one) and the node then holds 3.
- AC-2: the same node with every live writer above 75% of its cap lets go of nothing.
- AC-3: with the setting off, a pass lets go of nothing even where AC-1 would.
- AC-4: not nearly full and not above the mean, a pass lets go of nothing and makes no request.
- AC-5: far above the mean with a member below it, a pass lets go of shards down to 1.25 times the mean, at most a tenth of the cap a pass.
- AC-6: a handed-off shard's head is given back, and its next write opens it again with every earlier document.
- AC-7: the lease a node renews carries `held` and `cap`.
- AC-8: the setting is registered, default false, and `OSS_HAND_OFF` sets it and refuses a value that is not true or false.

## Test plan

`crates/node/tests/writer_hand_off.rs` over a role on the emulator with hand-made leases (AC-1 to AC-6); a lease-write test in `crates/node/src/writer/lease.rs` or the boot tests (AC-7); the settings registry and `crates/node/src/boot/config/tests.rs` (AC-8).

## S3 request budget

Unchanged for the lease: `held` and `cap` ride in the renewal PUT that was made anyway. A pass over a node that should not hand off makes no request. Each shard handed off costs what a cap eviction costs (a publish when unpublished, a head read and a swap); it scales with shards handed off, which the per-pass bound limits, never with elapsed time or idle shards.

## Switch

`serverless.activation.hand_off.enabled`, default false. It flips on once a multi-node run shows a node joining a full fleet fills without write p99 or request counts moving.

## Risks

- Without steering, a write for a handed-off shard still reaches the node that let it go if that is where the client sent it, and that node takes it back; the pass then repeats. The per-pass bound and the eviction grace limit the churn, and steering is the next piece.
- The mean and room decisions read leases up to a refresh old.

# Reader slow start

## Serves

Huy's ask: a joining reader must not take its whole share of shards at once with an empty cache. Builds on reader ranking (`placement::readers_for`, Java `ReaderPlacement.candidatesFor`) and the lease's `search` role; works beside warm copy (reader-warm-copy). Implemented in both the Rust build and the Java shell, byte for byte the same rule.

## Change

A search node's lease gains one optional field, `search_since`: epoch milliseconds at which the node first advertised the `search` role (with warm copy, when it turned ready). A lease without it is a fully ramped node, so every existing lease and every node that does not publish it ranks as before.

Every ranker computes the same ramp from the lease and its own clock, with integers only. The ramp is **decided by the joining node and published in its lease**; time only bounds it.

- The lease carries `search_since` (above) and `ramp_permille`, the joiner's own judgement of how warm it is, 0 to 1000. Absent `ramp_permille`: only the time bounds apply.
- `floor_curve(now) = clamp((now - since) * 1000 / min_duration, 0, 1000)`: the ramp can never be faster than this, however warm the joiner says it is. `ceiling_curve(now)` is the same over `max_duration`: the ramp can never be slower than this, however cold the joiner says it is, so a stuck, idle or lying joiner is fully ramped by `max_duration`.
- `ramp(now) = max(ceiling_curve, min(published, floor_curve))`. With no published value, `ramp = ceiling_curve`. A `max_duration <= 0`, absent `since` or future `since`: 1000 (switch off).
- The joiner's controller runs at each lease renewal. It starts at the first step (100), because a node ranked behind every shard gets no search and could otherwise never gather evidence; the floor curve still holds that back. It reads the searches this node has answered and the store GET-class requests it has made since the last window it judged: when a window holds at least 20 searches and averaged at most two store GETs a search, it raises `ramp_permille` by 100; a window with fewer searches keeps collecting (no evidence, so idle indices cost nothing and the ceiling alone carries the ramp); a colder window holds the ramp (newly admitted shards are cold by nature, and the ceiling guarantees the end). The ramp is never lowered. The numbers are constants of the controller; a node that does not count its store usage does not publish `ramp_permille`, so time alone bounds it. The signal is the GET count, which the node already keeps, not a latency, which it does not. The Rust node counts the shard searches forwarded to it, the Java node every shard query it runs; a Rust node that mostly answers its own searches advances on time alone. The lease says the two fields only while the switch is on, so with it off a lease is byte-identical to before.

A shard's admission value for a node is `murmur3(node_id + "/" + index + "~" + shard) mod 1000` (non-negative, Java `floorMod`). The node is *admitted* for the shard when `admission < ramp`. `readers_for` / `candidatesFor` rank admitted candidates first, each group in the existing order (higher weight, then lower id). A node not yet admitted is ranked after every admitted one, so a joiner is still a last-resort fallback.

Because the admission value is fixed per (node, shard), a shard admitted to the joiner stays admitted as the ramp rises, and the shards that move are only those moving to the joiner (a shard's order among the other nodes is untouched). About `ramp/1000` of the joiner's final share has moved at each moment.

Hot-shard spread and zone preference rank through the same function, so a not-yet-admitted joiner is outside the spread's top readers unless a shard needs more readers than there are admitted ones. Writers (single owner) are not ranked this way. Warm copy's hot set uses the same ranking, so a joiner asks peers for the shards admitted to it now and for no others.

Nodes may disagree (clock skew, a lease seen a refresh late): the cost is a cache miss on a shard, never a wrong answer, as for all placement.

Not changed: the owner choice, lease expiry, the search path past ranking, a node with the switch off.

## Acceptance criteria

- AC-1: `ramp` is 0 at `since` with the joiner saying 0 or nothing; never above `floor_curve` however high the published value; never below `ceiling_curve` however low; 1000 for no `since`, for `max_duration <= 0` and for a `since` in the future; no overflow at `i64` extremes. Rust and Java agree on a shared fixture of (now, since, duration) rows.
- AC-2: the lease round-trips `search_since` and `ramp_permille` (a value outside 0 to 1000 is clamped on read); a lease without it is byte-identical to today's, in Rust and Java; each side reads a lease the other wrote, and ignores the field when it does not know it.
- AC-3: with a ramp of 0 or the switch off, `readers_for` / `candidatesFor` answers exactly as before on the existing placement fixtures (the old tests pass unchanged).
- AC-4: over 2,000 seeded shards and a fleet of 12, with one joiner at ramp r: the joiner is first for about r/1000 of the shards it would hold at full weight (within 5 points), every shard it holds at ramp r it also holds at any higher ramp, and no shard's order among the other 12 nodes changes.
- AC-5: a joiner not admitted for a shard is ranked after all admitted candidates, and appears in the answer only when `limit` exceeds their number.
- AC-6: Rust and Java produce the same ranked list for a shared fixture of fleets, ramps and shards (`fixtures/placement/`), checked by each side's own test against one file.
- AC-7: `HotShardSpread` and `ZonePreference` over a ranked list that holds a not-yet-admitted joiner never pick it while an admitted reader is eligible.
- AC-8a: the joiner's controller, fed fake totals: raises by 100 per qualifying window, keeps collecting under 20 searches, holds on a cold window, stops at 1000; and the lease says the ramp the node set. What the rankers use never exceeds the floor curve and reaches 1000 by `max_duration` with no traffic at all (AC-1).
- AC-8: the lease sets `search_since` when the node first advertises `search`, and with warm copy on, at ready and not before; a lease renewal keeps the value.
- AC-9: provisional, emulator with latency: a joiner added to 12 warm readers, with the ramp on against off, its first-search GETs and p99 over the first minutes and the peers' p99, in `VERIFIED.md`; the default follows the build rule.

## Test plan

AC-1, AC-4, AC-5: unit tests in `oss-cluster` (Rust) and `ReaderPlacementTests` (Java), first run red against the unchanged ranking. AC-2: `oss-format` lease tests with `fixtures/node-leases/`; Java `NodeLeaseTests`. AC-3: the existing placement tests untouched. AC-6: one fixture file generated by the Java, read by both. AC-7: `spread` and `zones` tests. AC-8: node tests with a fake clock. AC-9: an ignored evidence test.

## S3 request budget

LIST 0, HEAD 0, PUT 0, GET 0 added. The two fields ride in the lease the node already writes (about 60 bytes, inside the lease size bound); the ramp is computed from leases already read. A shard moves to the joiner later than it would have, so GETs fall only by what warm copy and the spread already save. Unchanged request counts in the budget table.

## Alternatives considered

| Option | Agreement across nodes | Extra requests | Correctness | Verdict |
|---|---|---|---|---|
| Fixed time ramp | exact (lease time) | none | cache miss only | simple but blind: too slow for a joiner warmed by copy, too fast for a cold large fleet |
| **Joiner-published ramp, bounded by a floor and ceiling curve (chosen)** | exact: everyone reads the same lease value; lease refresh lag only | none (lease already written) | cache miss only; floor stops a lying or fast joiner, ceiling stops a stuck one | adapts to warm copy, cache size and load |
| Per-shard readiness: the joiner serves only shards it is warm for | none: peers cannot know without a shard list in the lease, too big for 2 KiB | a refused search costs a round trip per shard, or a probe request | exact answer from the joiner | rejected as the main rule; the joiner may still refuse (503) a search it cannot serve, as the warm-up does today |
| Coordinators learn each reader's latency and weight it (load-aware) | none: every coordinator sees different traffic | none | miss only | rejected: nodes would disagree, which is what the lease-derived weight avoids |

## Switch

`serverless.search.reader_ramp.max_duration` (cluster setting, Rust and Java; env `OSS_READER_RAMP_MAX` in Rust), default `0` (off); `serverless.search.reader_ramp.min_duration` (default 30 s); the controller's step, GETs-per-search target and minimum window are constants of the controller until a measurement says they need to move. `search_since` and `ramp_permille` are published whether or not the switch is on. Default to flip only when AC-9 shows no search regression and no extra requests; that flip is its own PR.

## Risks

A ranker with the switch on and one with it off disagree for the ramp's length: cache misses only. A node whose clock runs far ahead treats a joiner as ramped early; the joiner then serves its full share cold, which is today's behaviour. A joiner restarted inside the ramp restarts it (`search_since` resets), which is wanted.

# Placement: every node computes the same owners from the same leases

## Serves

Roadmap R4.2 ([plan](../../roadmap.md)): which node owns a shard's writes, which readers serve its searches, who takes a dead node's shards and when a writer hands shards off, computed by Rust nodes exactly as Java nodes compute them, so a mixed fleet agrees. [ADR 0015](../../adr/0015-cluster-crate-and-node-leases.md) (the `oss-cluster` crate); principles: no leader, the shard head's compare-and-swap decides ownership.

## Change

- `oss-cluster` gains `placement`, a port of the shell's `cluster/Rendezvous`, `ReaderPlacement`, `HotShardSpread`, `ZonePreference`, `Zones`, `ShardAssignment` and the pure decisions of `node/FleetPlacement` (`fleetSize`, `nearlyFull`, `memberWithRoom`, `takeoverWinner`, `fleetMeanHeld`, `aboveFleetShare`, `memberBelowMean`, `handOffTarget`, `writerFor`), with OpenSearch's `Murmur3HashFunction.hash(String)` and Java's `String.hashCode`.
- Pure computation over the leases a node holds: no store request, no lock, no state shared between nodes. The hot-shard spread counts this node's own searches in memory, bounded at 50,000 shards.
- Not changed: no node calls placement yet (the writer's takeovers and the coordinator's search routing will). `takeoverBelongsElsewhere` needs shard heads and comes with them. The four reader-placement settings are constructor arguments with the Java's defaults until a node wires them, as the lease lifetime is.

## Acceptance criteria

- AC-1: `murmur3` and `java_hash_code` equal the Java's for every string in `fixtures/placement/hashes.txt` (empty, one to five bytes, accented, CJK, a surrogate pair, a long key), and a key hashed in parts equals its join.
- AC-2: `owner` names the Java's `Rendezvous.owner` for each of the 80 recorded (key, candidates) pairs, ties broken by Java's id order; no candidates is `None`.
- AC-3: `readers_for` returns the Java's `ReaderPlacement.candidatesFor` for each of the 180 recorded (index, shard, role, limit) cases over the recorded fleet, keeping at most `limit` entries.
- AC-4: `ZonePreference::order` returns the Java's order for each of the 360 recorded cases (zone, shard rate, readers busy), more than 50 of which move a zone-local reader first.
- AC-5: each of the 1,017 recorded fleet decisions, and each decision for 400 random small fleets with loads drawn around every threshold, (fleet size, member with room, mean held, member below mean, hand-off target, writer for a shard, and the takeover winner with and without overcommit, for a dead owner in the view, departed with a known zone, and departed unknown) equals the Java's.
- AC-6: the hot-shard spread: a cold shard keeps placement's order; the rate sets the width up to `max_readers` and decays; of the width, two random picks prefer the reader with fewer searches running; every reader appears once; past 50,000 tracked shards the cold ones are dropped.

## Test plan

- AC-1 to AC-5: `crates/cluster/tests/placement/{java,fleet}.rs` over `fixtures/placement/`, recorded by `Placements.java` from the shell's classes compiled unchanged (FleetPlacement's decisions copied, since that class needs the metadata plane). Fails if a hash, tie-break, zone rule or room check differs.
- AC-6: `crates/cluster/tests/placement/spread.rs`, by properties, since the picks are random.
- Mutants over the diff; planted defects: a tie broken the other way, a takeover start from murmur3 instead of `hashCode`.

## S3 request budget

| Operation | Requests |
|---|---|
| any placement decision | 0 |
| a departed owner's zone, first takeover only | 1 GET (its lease, through `Membership::read`), then remembered |

## Switch

None: a port with nothing to compare against, making no requests.

## Risks

- A hash or order that differs from the Java's would make Rust and Java nodes pick different owners; correctness still holds (the shard head's compare-and-swap picks one), but takeovers would contend. AC-1 to AC-5 compare every recorded answer.
- `readers_for` and `ZonePreference` rank expired members as the Java does; callers filter by liveness where the Java's callers do.

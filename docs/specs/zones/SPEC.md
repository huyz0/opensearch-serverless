# Zones

## Serves

Roadmap R12.4 ([plan](../../roadmap.md)): a node's zone, the preference that keeps a hot shard's searches in the coordinator's zone, and the meter of bytes exchanged across zones, as the Java shell has them (`cluster/Zones`, `cluster/ZonePreference`, `transport/ZoneTraffic`). Cross-zone traffic is billed by the byte; the object store is free from every zone in a region.

## Change

- `oss-cluster` gains `zones`: `attribute` and `local_zone` read the zone from stock OpenSearch's awareness attribute (`node.attr.zone`, or the first of `cluster.routing.allocation.awareness.attributes`); `same`, `crosses`, `normalise` and `zone_of` move here from `placement`, with `ZonePreference` (re-exported from `placement` unchanged); `ZoneTraffic` counts payload bytes per path (same zone, cross zone, unknown) and per action, with a `stats()` snapshot in the Java's `zone_traffic` shape.
- The lease already carries the zone byte-for-byte as the Java's (`oss_format::NodeLease::set_zone`, recorded fixtures); the lease format does not change.
- Not changed: no node forwards searches yet (the coordinator crate is empty), so nothing records into the meter and no stats endpoint renders it; the forward path and `/_serverless/stats` wire it when they exist. The zone-local settings (`serverless.search.zone_local.*`) stay constructor arguments with the Java's defaults until node wiring, as the other reader-placement settings do (the settings registry has no float kind yet). Cross-zone compression and same-zone warm copy come with the transport callers that use them.

## Acceptance criteria

- AC-1: `attribute` and `local_zone` answer as the Java's `Zones.attribute` and `Zones.localZone` for: `node.attr.zone` set, unset and blank; a renamed awareness attribute given as a comma list and as the `key.0`, `key.1` array form; a blank first entry skipped; an awareness list of only blanks falling back to `zone`.
- AC-2: a zone set on a node's lease survives renewal through `Membership` and is read by a peer's refresh through the lease digest, on the emulator; a lease with no zone is read as none. `Path::to_peer` classifies from it.
- AC-3: `ZoneTraffic` classifies as `ZoneTraffic.pathBetween` (either side unknown is `Unknown`), ignores zero bytes, totals and breaks down by action sorted by name, resets, counts exactly under eight concurrent threads, and holds at most `MAX_ACTIONS` names per path with the overflow counted under `_other` and the total still exact.
- AC-4: the Java's `ZoneCostMeterTests` fleet, modelled over placement (three zones, two search nodes per zone that coordinate and read, six shards, twenty rounds of a search from every node over every shard, a shard read on the coordinator itself moving no bytes): with the zone preference on, cross-zone search bytes are at most 5% of those with it off (`min_per_second` = `f64::MAX`), the Java's measured -95%; a fleet with no zones records everything as unknown and nothing as cross-zone.

## Test plan

- AC-1, AC-3: `crates/cluster/tests/zones/{attribute,traffic}.rs`. A missing array-form read, or the comma split not trimming, fails AC-1; a counted zero, an unknown counted as cross, or a dropped overflow fails AC-3.
- AC-2: `crates/cluster/tests/leases/zone.rs` on the emulator, with the lease tests' clock.
- AC-4: `crates/cluster/tests/zones/cost.rs`. A preference that never moves a local reader first leaves the ratio near the fleet's two-thirds and fails.

## S3 request budget

Unchanged: zero requests. The zone travels in the lease and digest a node already writes and reads.

## Switch

None new. The preference's behaviour is the Java's, with its defaults (`min_per_second` 1.0, `spill_in_flight` 8); the settings that override them land with node wiring.

## Risks

- A zone read differently from the Java's would split a mixed fleet's view of who is local; it costs bytes, never correctness (placement is advice). AC-1 pins the reading.
- AC-4 is a model over placement, not a measurement across a network: it shows the routing choice; the byte counts come from real forwards once the coordinator exists.

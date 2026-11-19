# The coordinator counts the bytes it exchanges with peers by zone

## Serves

Parity plan R12.4 follow-up (zones): the meter in `oss-cluster::zones` counts nothing until a caller records into it; the coordinator is the first caller, on every shard call of a fan-out.

## Change

`oss-coordinator` gains `zone_meter::ZoneMeter`, built from the node's zone, the lease members it knows and a shared `ZoneTraffic`. Its `exchange` wraps one call to a peer: it classifies the peer by the zone its lease advertises, counts the request bytes before the call (a request that went out is billed even when the call fails), and counts the answer bytes of a successful call. It does not change the call, its result or its error, and it adds no request. The per-shard call stays the node assembly's; the node renders `ZoneTraffic::stats` under `zone_traffic` in its own stats work.

## Acceptance criteria

- AC-1: an exchange with a peer in the same zone counts request and answer bytes under `SameZone`; with a peer in another zone, under `CrossZone`, broken down by the action name.
- AC-2: when either side declares no zone, or the peer has no lease among the members, the bytes count under `Unknown`, never under a zone.
- AC-3: a failed call counts its request bytes and no answer bytes, and returns its error unchanged.
- AC-4: a successful call returns its answer unchanged, and the call runs exactly once.
- AC-5: a fan-out over shards in three zones through `fanout::run` counts each exchange once, so the totals equal the sum of the bytes handed to the calls, split by path.

## Test plan

`crates/coordinator/tests/zone_meter.rs`: one test per criterion. AC-5 runs eight concurrent calls and compares each path's total with the sum computed from the inputs.

## S3 request budget

Zero: the wrapper makes no blob request and adds none to the call it wraps.

## Switch

None: counting costs one short lock per exchange and its downside is negligible; the cost of the lock was recorded with the meter and is revisited if the forward path shows contention.

## Risks

Request and answer sizes are the caller's to report; a caller that reports wire bytes instead of payload bytes makes the count incomparable across settings. The tests pin payload bytes.

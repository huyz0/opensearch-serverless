# A meter measures how long a write takes to be visible on another node

## Serves

Parity plan R9.4 (freshness meter): ack-to-visible p99 within 5% of the Java shell. Builds on the tail overlay in a booted node ([node-boot-tail](../node-boot-tail/SPEC.md)) and the `OSS_TAIL_OVERLAY` setting. The write route's wait is [node-refresh-wait](../node-refresh-wait/SPEC.md).

## Change

A slow-tier test, `crates/node/tests/freshness_meter.rs`, boots three real nodes on the latency-injecting S3 front end of the cluster suite, with the tail overlay on, and meters single-document writes:

- Each sample indexes one document through the first node, then searches the other two in turn until each counts it (one of them may own the shard; the rig has no owner lookup and the meter adds none). Two series: `refresh=false` and `refresh=true`.
- **Visible** is the time from the write's request until both other nodes have answered a search that counts the document: the later of the two. **Ack** is the time to the write's answer. A node whose first search after the ack does not count the document is a **miss** (counted per node, so two per sample at most).
- The test prints one `FRESHMETER` line per series: ack and visible p50 / p99 / max, misses, and the blob requests per write by kind. Percentiles are nearest-rank over the samples.
- The cluster rig takes extra node settings, so a test can boot nodes with the overlay on. Nothing else changes in the rig or in `src`.

The Java reference is the shell's `REFRESHMETER` run of 2026-11-12 on the same latency profile (GET 20-30, PUT 40-60, LIST 40-70 ms), mode `serving` (`everywhere` is the shell's default, +3 ms p50 on shards searched less than every 500 ms): `refresh=true` ack p99 519.3 ms on RustFS and 528.9 ms on the filesystem, 0 misses of 240. In the shell a refreshing write answers only when the readers have it, so its ack is its visible time; the Rust bar is therefore the larger, 528.9 ms, plus 5%.

## Acceptance criteria

- AC-1: the meter indexes N documents per series through one node and counts each by a search on each of the two other nodes; a document not counted by both nodes within 10 s fails the test.
- AC-2: the printed percentiles are nearest-rank over the samples taken: p99 of 100 samples is the 99th smallest.
- AC-3: `refresh=false` documents are visible within the bar: visible p99 <= 1.05 x 528.9 ms.
- AC-4: `refresh=true` documents are visible within the same bar, and the line reports the misses; the miss count is printed, and under `serving` and `everywhere` (where the write route waits for its readers) a nonzero count fails the test. `METER_TAIL` picks the refresh visibility the cluster boots with (default `everywhere`); under `owner` the misses are reported, not asserted.
- AC-5: the meter writes only through the REST routes and reads only through them; it adds no hook to a node.

## Test plan

`crates/node/tests/freshness_meter.rs`, `#[ignore = "slow"]`: the percentile function has unit tests that run in the normal tier (AC-2: known samples, ranks at the edges, one sample, an empty list is refused). AC-1, AC-3, AC-4 are the ignored meter run. The percentile tests are written first and seen to fail on a stub.

## S3 request budget

Unchanged: the meter makes the requests its writes and searches make and adds none. The per-write request counts it prints are for reading, not a budget row.

## Switch

None: a test only. The overlay it runs with is `OSS_TAIL_OVERLAY`, on by default, as [overlay-default-on](../overlay-default-on/SPEC.md) records; `METER_OVERLAY=false` runs the meter with it off.

## Risks

- The emulator's tail (no 1-in-50 slow requests) is thinner than the shell's meter, so p99 here is flattering; every number is provisional (emulator, not RustFS).
- The index is created with an explicit mapping. A booted node's overlay cannot build over an index whose fields are all dynamic: the commit carries no mapping, so the build refuses (`no mapper for field`) and readers see only published commits. The meter does not cover that case; it is a gap in the overlay builder, reported to the coordinator, not hidden here.

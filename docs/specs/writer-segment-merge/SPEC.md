# The writer merges a shard's small segments

## Serves

Parity plan R15, the Java-retire gate, and every cold open. The writer never sets a merge policy (`oss_writer::index::knn::open_writer` leaves `IndexWriter::set_merge_policy` at its default of none), and every refresh of a shard (each acknowledged bulk forces one) writes a segment. A shard's commit therefore lists a segment for every refresh since it was created. A reader, a taking-over writer or a Java coordinator opening the shard reads each segment's files in turn, so a cold open costs time proportional to the shard's write history. In the wide mixed-fleet soak (4 shards, two writers, 2 Java and 2 Rust nodes) the store held 797 segment blobs for 2,700 documents after the first cycle, and a Java node's `_search` sat in its own cold open for over six minutes while a Rust owner serving the Java shard action blocked in its own open ([ADR 0058](../../adr/0059-the-writer-merges-a-shards-small-segments.md)).

## Change

A shard's writer opens its `IndexWriter` with a tiered merge policy. A refresh's commit consults it and merges small segments inside the shard's in-memory directory, as the Java engine does. The next publish uploads the merged segment instead of the segments it replaced, and the manifest lists only live segments. Blobs a merge replaces are not deleted by this change: the janitor judges only older terms ([janitor-gc](../janitor-gc/SPEC.md)), so they stay until a takeover, and no pin is needed for readers, point-in-time views or snapshots that still name them.

## Acceptance criteria

- AC-1: with merging on, a shard refreshed 200 times with one document each, then refreshed with `max_full_flush_merge_wait_millis` unbounded until a refresh merges nothing, commits at most 24 segments; with merging off it commits 200.
- AC-2: after merges, every document is found exactly once by id and by `match_all`, an update made before a merge reads at its newest version, a delete made before a merge stays deleted and a soft-deleted document stays out of search, and sequence numbers, versions and the commit's local checkpoint and max sequence number equal those of the same operations with merging off. A shard dropped after a merge and before a publish is taken over from the last published commit and the log, and holds every acknowledged document.
- AC-3: a publish after a merge uploads the merged segment's blob and not the replaced segments'; the manifest lists no replaced segment; the publish makes no more blob requests than the same publish with merging off, with node packs off and with node packs on.
- AC-4: a merge reserves twice its input bytes from the node's memory pool before it runs and releases them when it ends; with the pool unable to grant them the merge is skipped, the refresh succeeds and the merge is tried at the next refresh; with eight shards due to merge at once, at most one merges at a time and the pool's peak never exceeds one merge's reservation.
- AC-5: with `serverless.writer.merge.enabled` off, commits are what they are today: the existing tests that pin segment counts run unchanged, and a new test shows 200 refreshes leave 200 segments.
- AC-6: a Java node takes over a shard whose commit holds merged segments, including a k-NN index, and finds every document (mixed fleet, `crates/node/tests/mixed_fleet.rs`).
- AC-7: in the wide soak, each shard's latest commit lists at most 24 segments at the end of every cycle, and every answer of every node has `_shards.failed == 0` within `HANDOVER_SLACK_SECS` of the judge starting ([java-search-shard-action](../java-search-shard-action/SPEC.md) AC-10).
- AC-8: refresh time and bulk acknowledgement time (p50, p99), and the time of a search and of a cold open of the shard, with merging on are each within 5% of off or better, over the same load on an emulator (provisional); the default of the switch follows this result.
- AC-9: a merge makes no blob request: `docs/request-budgets.md` has the exact row `writer.merge.requests` = 0, and the test fails on one request above it.
- AC-10: a reader following pushed commits across a merge answers correctly at every commit and fetches the merged segment once; a point-in-time view and a snapshot taken before a merge read and restore after it, because no blob a merge replaced is deleted by this change.
- AC-11: soft-deletes retention stays `None`: a merge keeps a soft-deleted document, and the spec records the bytes a shard holds after a merge with updates against without them.

## S3 request budget

`writer.merge.requests`: a refresh that merges makes no blob request. A publish uploads one blob per live segment it has not uploaded; merging does not raise that count and lowers it when a merge covers segments not yet uploaded. Cost in bytes: a merged segment is uploaded once, so a byte is uploaded at most once per tier it passes through (write amplification of about the number of tiers, a log of the shard's size over the floor); the spec measures it.

## Switch

`serverless.writer.merge.enabled`, with `segments_per_tier` (8), `max_merge_at_once` (10), `max_merged_segment_size` (512 MiB) and `max_full_flush_merge_wait_millis` (500) as settings. Default on if AC-8 holds, off if not; see the ADR. The existing tests pin the off state and new tests cover on; both run.

## Out of scope

Reclaiming replaced blobs in the live term (needs pins through the janitor; its own ADR), dropping soft-deleted history, force merge by request, merging across shards, merging on a reader, and a merge scheduler off the refresh path (a follow-up if AC-8 fails).

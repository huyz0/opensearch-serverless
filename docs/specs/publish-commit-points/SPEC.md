# A publish always carries its commit point

## Serves

Parity plan R7: a published commit a reader can open (issue #218, found by the node cluster suite).

## Change

A refresh that wrote a new `segments_N` and no new segment file (a shard refreshed with nothing to flush, or a commit that only changes user data) published a manifest without that `segments_N`: commit points rode only in the last fresh segment's blob, and with no fresh segment they were dropped. The publisher now uploads them as a blob of their own when no fresh segment can carry them. Nothing else changes: a publish with a fresh segment still makes one blob PUT and one swap.

## Acceptance criteria

- AC-1: a publish of a commit with no fresh segment file but a new `segments_N` names that `segments_N` in its manifest, and the manifest names every file of the commit.
- AC-2: that publish makes one blob PUT and one register swap, and no read or LIST.
- AC-3: a publish with a fresh segment still makes one blob PUT and one swap (unchanged).

## Test plan

`crates/writer/tests/it/publish.rs`: `a_commit_with_no_fresh_segment_still_publishes_its_commit_point` (red before the change: the manifest lacks `segments_N`).

## S3 request budget

A publish with a fresh segment: unchanged. A publish with none: one blob PUT (the commit point) and one swap, where the old code made one swap and a manifest a reader could not open.

## Switch

None: the old behaviour is a defect with no upside.

## Risks

A commit point blob per no-change publish: bounded by the publish rate, and one PUT per publish is what a fresh segment costs too.

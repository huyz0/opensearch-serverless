# Janitor: the orphan-shard sweep

## Serves

R8.4 ([ADR 0028](../../adr/0028-orphan-shard-sweep.md), [ADR 0020](../../adr/0020-janitor-crate.md)): the backstop for a shard container whose index is gone.

## Change

`oss_janitor::orphans::Sweeper::slice(budget)` runs one slice of the resumable sweep, in the ADR's order: it reads the cursor register and lists the next `budget` shard containers strictly after it, reads the descriptors of their indexes, reads the pins if any container is a candidate, claims the page with one conditional swap, then deletes each candidate no pin names. `BlobStore::list_children` lists the child prefixes of a prefix (default: one LIST a child). Off unless `serverless.janitor.orphan_slice_budget` is above 0; runs once every `serverless.janitor.orphan_slice_interval` on the one node the rendezvous names; pins are capped by `serverless.janitor.orphan_max_pins` and `serverless.janitor.orphan_max_pin_records`; dry run counts and deletes nothing. A page that fails three times running is skipped and counted.

Not in this change: the node's timer that calls a slice, the Rust snapshot writer (R14.2: the sweep reads the Java's snapshot records), and the S3 delimiter listing.

## Acceptance criteria

- AC-1: with a budget of 0 a node registers no timer and its meter reads zero from startup, and `slice(0)` is refused.
- AC-2: a container of an index that has no descriptor, or a descriptor with another uuid, is deleted with its blobs; one of a live index is left.
- AC-3: a container whose name is not `<index>#<uuid>#<shard>` with a valid index name, a well-formed uuid and a shard in range is left.
- AC-4: a descriptor that cannot be read fails the slice and deletes nothing.
- AC-5: a shard pinned by a point in time or by a snapshot is left; a pin that cannot be read, a record over the read buffer, more pins than `orphan_max_pins` or more records than `orphan_max_pin_records` stops the slice with nothing deleted and the cursor unmoved.
- AC-5b: a capture in progress that names the index pins every shard of it below its shard count.
- AC-5c: a pin registered between the descriptor read and the pins' read, and one between the pins' read and the delete, never lets a pinned shard be deleted; a descriptor created after the page was listed is not deleted.
- AC-5d: `slice` has no constructor with a "no pins" source; a snapshot record in the Java's format, from a recorded Java run, pins its shard.
- AC-6: a slice that loses its swap to another writer of the cursor (a Java node, simulated) stops after its read, its listing, its descriptors and one failed swap, counts it, and moves nothing; the first-ranked node with a budget of 0 sweeps nothing and no other node takes over.
- AC-7: slices walk every container in order and the last writes an empty cursor; the next starts over.
- AC-8: under dry run a slice counts the orphans and deletes nothing.
- AC-9: a slice's requests equal the ADR's table on the emulator, for the portable form, with no orphan, with one orphan, and under dry run; a page with no candidate reads no pin.
- AC-10: a container whose name is not a validated `<index>#<uuid>#<shard>` is left; a blob already gone counts as deleted; the cursor value is the shell's (a test advances it alternately from a recorded Java value).
- AC-11: a page whose pins cannot be read fails the slice twice with the cursor unmoved and is skipped, counted and logged at the third; a node that is not the sweeper makes no request.

## Test plan

Emulator tests with exact request counts, one per criterion, each seen failing first.

## S3 request budget

The table in ADR 0028. A budget row `janitor.orphan_slice.requests` is added.

## Switch

`serverless.janitor.orphan_slice_budget`, default 0.

## Risks

A pin source that cannot say would let a sweep delete a snapshotted shard: `slice` takes sources with no "none" form, and a source that cannot read says everything is pinned.

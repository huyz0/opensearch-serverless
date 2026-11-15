# Tombstone sweep and the conditional-delete probe

## Serves

The plan's metadata-writes milestone, second part: deletes that stay safe on any store, and tombstones that do not pile up forever. Follows [descriptor-store](../descriptor-store/SPEC.md), whose `DeleteMode::Conditional` was trusted as configured. Invariant I-1 (every store call metered), the fencing rule (a delete applies only to the generation it claimed), and the S3 rule (the sweep's LIST is bounded by what it removes, and carries `// LIST:`).

## Change

Decision record: [ADR 0008](../../adr/0008-metadata-crate.md) (Accepted).

Ported from huyz0/OpenSearch `serverless/shell/src/main/java/org/opensearch/serverless/metadata/` at `3920dc36`: `ConditionalDeleteProbe.java`, `TombstoneSweep.java`, and `Tombstones.tombstoneBucketStart`.

- `DeleteMode::probe(store, base)`: the Java's probe, under `<base>probe/conditional-delete-<random>`. It creates a register, reads it back (so a store that remembers versions knows its tag, as the Java does), replaces it at the next generation with a plain PUT the store does not see as its own swap (another writer, as the Java's second container), asks for a delete at the old generation, and answers `Conditional` only if that delete was refused, the register survived, and a delete at its current generation then removed it. Anything else, an error included, answers `Tombstone`, as the Java's `false`. The probe's register is deleted at the end either way.
- `DescriptorStore::sweep_tombstones`: removes tombstones whose quarantine has passed, only in `Conditional` mode (in `Tombstone` mode a late delete could land on a recreated name, so tombstones stay, as the Java's). It walks the markers under `tombstones/` in key order. Bucket names are fixed-width UTC hours, so they sort by time, and the walk stops at the first bucket too young to hold an expired name: it never lists a young bucket's markers. For each marker it reads the name's register and:
  - leaves it (marker deleted) when the register is gone, is no tombstone, or is a tombstone deleted after the marker's hour;
  - keeps it when the tombstone's quarantine has not passed by its own `deleted_at`, or another sweep claimed it under a minute ago;
  - otherwise claims it by swapping in `reaping_at`, deletes the register at the claimed generation if still inside the 10-second deadline, and deletes the marker.
  A bucket the caller does not own, or a name that is not a bucket, is skipped without reading its markers.
- `tombstone_bucket_start` in `oss-format`: a bucket's first millisecond, or none for a name that is not a valid hour.
- Not in this change: removing a filesystem store's emptied bucket directory (the `BlobStore` trait deletes keys, not directories), scheduling the sweep, listing names, change stripes.

## Acceptance criteria

- AC-1: the probe answers `Conditional` on the emulator, S3 over loopback and fs, and leaves no key under `probe/`; it answers `Tombstone` on a store whose conditional delete ignores its condition (a test store wrapping the emulator), on one that fails a request, and on one refusing every create.
- AC-2: the probe's requests are counted on each backend and stated in the test.
- AC-3: on each backend, a sweep in `Conditional` mode after the quarantine removes a deleted name's register and marker, and the name can be created again; before the quarantine it removes nothing and reads no marker of the young bucket; in `Tombstone` mode it sends no request.
- AC-4: stale markers (a name recreated live, deleted again in a later hour, or already gone) are deleted without touching the register; a tombstone another sweep claimed under a minute ago is kept, and one claimed over a minute ago is claimed afresh and removed; a claim found past the 10-second deadline is left claimed with its marker kept.
- AC-5: a bucket `owns_bucket` refuses, and a marker directory that is not an hour, are skipped: their registers are not read.
- AC-6: requests, counted on each backend: a sweep with nothing expired is 1 `List`; each name removed adds 1 `Get`, 1 `Put` and 2 `Delete` (plus 2 `GetRange` on fs, which reads the generation before its swap and its delete); each stale marker adds 1 `Get` and 1 `Delete`; markers are listed a page at a time, so a page size of 2 over 5 expired markers costs 3 `List`, and a skipped bucket is listed past rather than through.
- AC-7: `tombstone_bucket_start` is the inverse of `tombstone_bucket` on hour boundaries from 1970 to 9999, and none for a name of the wrong length, a non-digit, month 13, day 31 of a 30-day month, February 29 of a year that is not leap, and hour 24.

## Test plan

- AC-1 to AC-6: `crates/meta/tests/descriptors/` (new modules `probe.rs` and `sweep.rs`) over the three backends, with exact meter counts; a store wrapper that turns `delete_register` into a plain delete for the ignoring-store case. Seen failing against a planted sweep that skips the deadline check and a planted probe that skips its stale delete.
- AC-7: `crates/format/tests/records.rs`.

## S3 request budget

| Operation | PUT-class | GET-class | LIST | DELETE | Depth |
|---|---|---|---|---|---|
| probe (emulator, S3) | 2 | 3 | 0 | 3 | 8 |
| probe (fs) | 2 | 3 and 2 ranged | 0 | 2 | 8 |
| sweep, nothing expired | 0 | 0 | 1 | 0 | 1 |
| sweep, per name removed | 1 | 1 | 1 per page of markers | 2 | 4 |
| sweep, per stale marker | 0 | 1 | as above | 1 | 2 |

LIST scales with the markers of expired hours, which is deletes, never with live indexes. The Java lists the bucket directory, then each expired bucket; this lists the markers in one walk and stops at the first young bucket. The Java batches marker deletes (one DeleteObjects per 1,000); this sends one DELETE each, which S3 does not bill.

## Switch

None: the probe replaces trusting the configured mode, and the sweep runs only when a caller schedules it.

## Risks

- A sweep is one LIST per run even when nothing is due, as the Java's directory listing: a caller runs it hourly, not per index.
- The filesystem store keeps an emptied bucket's directory; on a single host that is one empty directory an hour.
- The probe runs once per store; an error during it picks tombstone deletes for the store's life, as the Java's. Safe, and tombstones then wait for a later process to sweep them.

# Bucket formats and placement against the Java shell

Status: verified against code the Java shell runs, not against a running shell.

## Intent

The bucket manifest layouts, the stripe and bucket id, and the placement ranking were each written from a reading of the Java and checked against an encoder written from the same reading. This spec holds them against outputs the Java classes themselves produced, so a mistaken reading shows up.

## Acceptance criteria

- AC-1: the JSON bucket manifest the Java `BucketManifest.toBytes` wrote is read, every member's commit parses, and writing it back gives the same bytes.
- AC-2: the compact manifest in each codec the Java writes (none, deflate, zstd, lz4) is read to the same members, whole and one member at a time; the JSON and compact forms read to the same commits.
- AC-3: the Java stripe of a group (including empty, non-ASCII and long groups) and its bucket id are the Rust ones.
- AC-4: for every bucket of the 16 stripes by four generations over a fleet with a revoked, an expired, a non-writer, an unreporting and an overloaded node, the Java ranking and its keep decision are the Rust ones; a late joiner is held down for the same time.
- AC-5: the register keys (`buckets/<id>/manifest`, `bucket-open/<n>`) and the bucket id rule are the Java's.
- AC-6: a manifest the Rust writes in each of its forms is read by the Java `BucketManifest.parse` (a one-off run recorded in `VERIFIED.md`, since no Java runs in the gate).

## Tests

`crates/format/tests/bucket_java_fixtures.rs`, `crates/writer/tests/it/bucket_placement_java.rs`, over `fixtures/bucket-manifests/` (see its README for how the files were made).

## S3 request budget

None: no store is touched.

## Known divergence

A commit whose recorded file length differs from its location's range is read by the Java and refused by the Rust (`length-disagrees.json`). The Java never writes one; the Rust refusal stays, so a corrupt member entry is not followed.

## Risks

- The generator ran the shell's bucket sources against OpenSearch 3.3.0 libraries, not the fork's own core. Only `BucketManifest`, `CompactManifestFormat`, `ManifestCodec`, `BucketPlacement` and `CommitManifest` ran; `BucketAssigner`'s two formulas were copied line for line.
- Nothing here runs a Java and a Rust node on one bucket; that is the mixed-fleet soak.

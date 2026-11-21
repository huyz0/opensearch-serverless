# Bucket manifest writer

## Serves

R11, small indices grouped in a bucket: the writer side starts with the bucket manifest as a value a node can build, change one member at a time and write back. Ported from the shell's `BucketManifest` (`Member`, `empty`, `withMember`, `withoutMember`, `atTerm`, `dataBytes`, `toBytes()`, `fromStream`) at huyz0/OpenSearch `429b4cbc`. The reader (`parse_member`) already exists; this is its inverse and the whole-manifest read a read-modify-write needs.

## Change

`oss_format::BucketManifest` holds a bucket id, the bucket head term and its members in uuid order, each a `BucketMember` (index name, entry version, commit as the JSON a shard's commit register holds). It parses the JSON encoding whole, edits by value (`with_member`, `without_member`, `at_term`), reports `data_bytes` (the sum of the recorded file lengths of every member's latest commit, what fills a bucket) and writes the JSON encoding `BucketManifest.toBytes()` writes: `{"bucket":…,"term":…,"members":{"<uuid>":{"index":…,"version":…,"commit":{…}}}}`, members by uuid, `version` only when positive, `commit` only when there is one. `BucketMember::first` and `BucketMember::advance` keep the version rule: a member's entry moves one version when its commit is swapped and at no other time.

Not here: the compact (`OSBM`) encoder and its whole-manifest decode (a compact register is refused as unsupported by `BucketManifest::parse` for now), the register swap, bucket heads and assignment.

Stricter than the Java, each refused: more than `MAX_BUCKET_MEMBERS` (1,024) members, a member uuid that is empty, a commit that `CommitManifest::parse` refuses, and a term past `i64`.

## Acceptance criteria

- AC-1: a manifest written and parsed again is the same manifest, with members in uuid order, a member with no commit (version 0, no `version` field) and one with a commit (`version` field, commit bytes kept as written).
- AC-2: the written bytes are exactly the Java's JSON shape for a known manifest (field order, no whitespace, escapes), and `parse_bucket_member` reads each member of it back.
- AC-3: `parse` reads a manifest the Java wrote (hand-written in `toBytes()`'s shape, fields in any order, unknown structures skipped) into the same members `parse_bucket_member` gives one by one.
- AC-4: `with_member` adds or replaces, `without_member` removes (a missing uuid changes nothing), `at_term` changes only the term, each leaving the other members as they were.
- AC-5: `BucketMember::first(name, None)` is version 0, with a commit version 1, and `advance` is the next version with the new commit; a member whose version and commit disagree is refused.
- AC-6: `data_bytes` is the sum of the recorded lengths of every member's commit, zero for members with no commit.
- AC-7: refused with an error, never a panic: a compact register, a manifest of 1,025 members, an empty uuid, a commit that is not a manifest, a bucket id that breaks the id rule, a term past `i64`, and every proper prefix of a written manifest.

## Tests

`crates/format/tests/bucket_manifest_writer.rs`.

## S3 request budget

None: pure encoding. The register swap is the caller's.

## Risks

- The writer's JSON is checked against the Java's shape as read here, not against Java-written bytes; the R7 differential suite is the first check.
- A whole manifest is held in memory, bounded by `MAX_BUCKET_MEMBERS` commits of at most the commit manifest's own cap.

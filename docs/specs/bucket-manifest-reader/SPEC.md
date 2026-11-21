# Bucket manifest reader

## Serves

R10.6, the last blocker of bucketed points in time: Rust could not read a bucketed index's commit, which lives in its bucket's manifest register (`buckets/<id>/manifest`), not in a shard register. Ported from the shell's `BucketManifest` (`parse`, `parseMember`, `fromStream`), `CompactManifestFormat` (`open`, `decodeMember`, `readCommit`) and `ManifestCodec` (`decompress`), `BucketStore` (`ROOT`, `MANIFEST`) and `BucketIds.validate`, at huyz0/OpenSearch `429b4cbc`.

## Change

`oss_format::bucket_manifest::parse_member(value, index_uuid)` reads one member's entry from a bucket manifest register value (the bytes after the generation) in either encoding the shell writes: the JSON one, and the compact one (`OSBM`, format 2) with codecs none, deflate, zstd and LZ4. It returns the bucket id, the bucket's term, and the member if present: its index name, its entry version, and its commit as the same JSON bytes a shard's commit register holds, so `CommitManifest::parse` and `ShardRef::resolve` read it unchanged. Other members are skipped without being decoded. `bucket_manifest_key(base, bucket)` derives the register's key.

Not here: writing a bucket manifest, bucket heads, and the capture and reader changes that use this (the next change).

Stricter than the Java, each refused as corrupt: a string past 1,024 characters, a commit past 8 MiB of names and locations, a body (none included) that is not the header's length, a decompressed length that is not the header's, a body past 64 MiB, a member entry that does not end where its recorded length says, a file whose length tag is the range's but whose location has no range, and a length tag of 3. A pruning digest and lengths for files the commit does not name are skipped, as `CommitManifest::parse` skips its digest.

## Acceptance criteria

- AC-1: a JSON bucket manifest gives the named member's index, version and commit, and the commit parses to the files the Java wrote; the other members' contents are never read.
- AC-2: a compact manifest with codec none gives the same member as the JSON one for the same data.
- AC-3: compact manifests compressed with deflate, zstd and LZ4 give the same member as uncompressed.
- AC-4: a uuid that is not a member gives the bucket and term and no member; a member with no commit has version 0 and no commit.
- AC-5: a compact commit's term, writer, write-log ordinal, stage sequence, node packs (delta coded), other pack prefixes, whole-blob locations and all three length forms come out as the Java wrote them.
- AC-6: bytes that break the format are refused with an error, never a panic: every proper prefix of a compact manifest, an unknown format or codec, a header length that lies or is past the cap, a string, pack or member count past the body, a string index out of range, and a member version and commit that disagree.
- AC-7: `bucket_manifest_key` is `buckets/<id>/manifest` under the base, and refuses an id that is empty, over 64 characters or holds anything but `a-z`, `0-9`, `-` and `_`.
- AC-8: a manifest of 1,000 members is read for one member and only that member's entry is decoded (the other members' uuids are compared, not kept); a count larger than the bytes left, a string past 1,024 characters, more packs than `MAX_FILES`, a header claiming more than its stream can hold, and a commit whose file names and locations decode past 8 MiB are each refused before the allocation they would size.

## Tests

`crates/format/tests/bucket_manifests.rs`. Manifests are built by a test-only encoder that follows the Java's `CompactManifestFormat.body` byte for byte, and by hand-written JSON in the shape of `BucketManifest.toBytes`; no Java process wrote them (the shell build is not available in this sandbox), which `VERIFIED.md` says.

## S3 request budget

None: pure parsing. The register read is one GET, the caller's.

## Risks

- Where a uuid is listed twice the first entry is read, as the Java's `decodeMember` does; the JSON form keeps the last, as its `TreeMap` does.
- A compact manifest carries every member of a bucket, and each read moves the whole register; the cap on the decompressed size bounds what a read can ask of memory.
- The Rust decoder is checked against an encoder written from the same reading of the Java; a mistaken reading would be shared. The first differential run against a shell-written bucket (the R7 suite) is the check.

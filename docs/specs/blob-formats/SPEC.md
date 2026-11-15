# Shell blob format readers

## Serves

Roadmap stage 2 ("read the Java shell's published commits and segment blobs, one blob per segment, node packs"), invariants I-14 and I-15, ADR [0004](../../adr/0004-bounded-memory-and-streaming.md).

## Change

- `oss-format` gains readers, ported from huyz0/OpenSearch `feature/serverlessshell` at `49f238ed`, for: index descriptors and the alias and tombstone records that share their key (`cluster/IndexDescriptor.java`, `metadata/DescriptorStore.java`, `metadata/Tombstones.java`), the register framing (`FsBlobRegisters.java`), commit manifests and file locations (`store/CommitManifest.java`, `store/FileLocation.java`), segment blob headers (`store/SegmentBlobs.java`), node pack headers (`store/NodePackBlob.java`) and the keys all of these live at (`metadata/RegisterMap.java`, `store/NodePackStore.java`, `store/SegmentPublisher.java`).
- The output a reader needs: from a descriptor register value and a commit register value, every Lucene file of the shard's commit as (blob key, offset, length). Keys are derived; nothing is listed.
- Pure parsing over byte slices: no I/O, no `oss-blob` dependency, no external crate. A small JSON pull reader replaces the Java's `XContentParser`; strings borrow from the input unless escaped.
- `fixtures/shell-blobs/` holds blobs the Java shell wrote, and the generator is a test on huyz0/OpenSearch branch `claude/project-thread-hti37o` (not on `feature/serverlessshell`).
- Not in this change: bucket manifests (`bucket/BucketManifest.java`, `CompactManifestFormat.java`, `ManifestCodec.java`): buckets are off by default (`serverless.buckets.enabled` false), and their codecs (deflate, zstd, LZ4) would be this crate's first external dependencies. A bucketed descriptor is refused with `NotFollowable`. Also not here: the pruning digest (skipped), the wire encoding of a manifest, staged commits held by a writer, any writer, and unit layouts beyond noting one is present.

## Acceptance criteria

- AC-1: the fixture descriptor reads as index `fixture`, uuid `fixture-uuid-0000000`, one shard, no bucket; its key is `indices/fixture`.
- AC-2: the fixture manifest reads as term 2, writer `gnPAlf__T_-qxystAAAAAA`, `wal_ordinal` 0, `stage_seq` 1, a digest present, and exactly its seven files with their recorded locations and lengths.
- AC-3: resolving the fixture manifest gives seven extents whose keys are the fixture's segment blob and node pack, each range inside its blob, and each range's bytes a whole Lucene file (codec header magic at its start, footer magic 16 bytes from its end).
- AC-4: the fixture segment blob's header names its four files smallest first, contiguous from the header's end to the blob's end, each at the range the manifest records.
- AC-5: the fixture node pack's header names its node, epoch, ordinal and one block of the fixture shard spanning the body to the pack's end; the block is a segment blob, and each node-packed manifest range equals the block's offset plus the file's offset in that blob.
- AC-6: every proper prefix of each fixture header is `Truncated`, and from the length prefix on, `needed` is the header's full length; every proper prefix of each register value is refused.
- AC-7: flipping any single bit of any fixture header or register never panics, and any header still accepted keeps every range past its header.
- AC-8: wrong magic, unknown version, negative or inconsistent header length, malformed JSON, missing required fields, a file named twice, a recorded length that disagrees with its range, and each malformed location form are refused as `Corrupt` or `Unsupported`.
- AC-9: limits hold at the limit and refuse one past: segment blob header 64 KiB and 1,024 files, node pack header 4 MiB and a block count the header's bytes cannot hold, manifest 16,384 files and 16,384 lengths, JSON nesting 64 levels.
- AC-10: keys are derived with the shell's layout and a caller's base; a name, uuid, node or blob part that is empty, a dot segment, or holds `/` (or `#` in a shard path) is refused rather than turned into a key.
- AC-11: a version 1 node pack (no index names) reads, and modified UTF-8 (`C0 80`, surrogate pairs) decodes while four-byte UTF-8 and lone surrogates are refused.
- AC-12: an alias, a tombstone, a bucketed index, and a shard past the shard count are `NotFollowable`.
- AC-14: skipping a field copies nothing: a descriptor whose escaped mapping string is 256 KiB parses with zero heap allocation, and a manifest's skipped strings cost nothing (a counting allocator in its own test binary).
- AC-13: every allocation in `crates/format/src` states its bound (`scripts/check-alloc.py`), and the gate passes.

## Test plan

Each criterion is a test in `crates/format/tests/`. AC-1 to AC-5 decode the Java's bytes and would catch an offset off by the header, a wrong key layout, or a misread length. AC-6 and AC-7 run over every prefix and every bit of the fixtures and would catch a slice index past the input or a length trusted before it is checked. AC-8 to AC-12 are tables of hand-made records built from the Java's rules. Mutation testing (`scripts/mutants.sh`) checks the tests notice changes to the readers.

## S3 request budget

None: no I/O. A reader built on this needs, to locate a shard's files, one GET of the descriptor register (usually cached) and one GET of the commit register: depth 2, no LIST, no HEAD unless a manifest records no length for a whole-blob file (the shell records every length). `resolve` returns one extent per file; files share blobs (the fixture's seven files are in two), so a reader groups extents by key and fetches each blob once, which its request-count test asserts.

## Switch

None: a new library with no caller yet; no behaviour of a running node changes.

## Risks

- The Java changes a format: the readers refuse it loudly (version checks, strict JSON) rather than misreading it, and regenerating the fixtures shows it.
- The readers are stricter than the Java in four places (duplicate names, length disagreement, block overlap, leftover header bytes), each noted in the module doc. A Java-written blob never trips them; if one did, it would be refused, not misread.
- The fixture is one small index on the filesystem store. The S3 container frames registers the same way (`S3BlobContainer`'s register body begins with its generation), which no fixture here shows.

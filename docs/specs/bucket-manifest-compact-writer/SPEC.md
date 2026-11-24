# Bucket manifest, compact form: write and read whole

## Serves

R11. The shell writes a bucket's manifest in the compact `OSBM` form by default (`serverless.buckets.manifest_codec` is `auto`), so a Rust writer that takes over or shares a bucket must read it whole and write it back in the same form, or every checkpoint rewrites a register the shell wrote compact as larger JSON ([ADR 0026](../../adr/0026-bucket-manifests-in-the-writer.md)). Ported from `CompactManifestFormat` (`body`, `writeCommit`, `encode`, `encodeAuto`, `frame`, `decode`, `Location.of`, `Pack.of`) and `ManifestCodec` (`compress`) at huyz0/OpenSearch `429b4cbc`. Builds on [bucket-manifest-writer](../bucket-manifest-writer/SPEC.md).

## Change

`BucketManifest::parse` reads either encoding whole (the compact one with every codec the member reader takes). `BucketManifest::encode_compact(codec)` writes `OSBM`, format 2: the string table (writers, pack paths, file stems and extensions, each once), the member index (uuid and entry length), then each member's entry with its commit as the shell writes it (packs once each, node packs as node and epoch and ordinal deltas, a file as stem, extension, tag, pack, offset, length; the three length forms). `encode_compact_auto(codec)` compresses only from 32 KiB of body and only if the result is at most 0.8 of it, as `encodeAuto` does. Codecs written: none, deflate, zstd. LZ4 is read, not written (the shell's `auto` writes zstd).

Not here: the register swap, member commit swap, bucket heads and assignment.

Differences from the Java, each stricter or lossy and said so: a string past 1,024 characters or a body past 64 MiB is refused on write (the reader refuses them); the pruning digest is not carried (the Rust commit reader skips it, so a manifest re-written by Rust drops it); a file whose length was not recorded but whose location has a range is written with the range's length (`CommitManifest` gives the range length for it). A given length that differs from its range's is never written: `CommitManifest` refuses such a commit, so only the none and of-range forms and a given length on a file without a range come out of a whole manifest.

## Acceptance criteria

- AC-1: for every codec written, `encode_compact` then `parse` is the same manifest, and `parse_bucket_member` reads each member from it.
- AC-2: the written body is byte for byte what the shell's encoder writes for the same manifest, checked against the test-only encoder that follows `CompactManifestFormat.body` (string table order, packs once each, deltas, tags, the three length forms, a node pack, a plain prefix pack, a whole location, files with no dot or a leading dot).
- AC-3: `parse` reads a compact manifest the member reader reads, member for member, for every codec, including LZ4.
- AC-4: `encode_compact_auto` writes codec none below 32 KiB of body, compresses a larger repetitive manifest, and writes none when the codec saves less than a fifth.
- AC-5: refused with an error, never a panic: every proper prefix of a compact manifest on `parse`, a member count past `MAX_BUCKET_MEMBERS`, a body that is not exactly its entries, an unknown codec, a string past 1,024 characters on write and a body past 64 MiB on write (LZ4 cannot be asked for: the codec type has no such value).
- AC-6: a location is taken apart only when it is a canonical range (`locate`), and a node pack only when putting it back gives the same text (`node_pack`); anything else is kept whole, as the shell's `Location.of` and `Pack.of` do. These are checked on the functions: `CommitManifest` refuses a raw location, so none reaches them from a whole manifest.

## Tests

`crates/format/tests/bucket_manifest_compact_writer.rs`, with the encoder in `tests/bucket_support`, and unit tests in `compact_write.rs` for `locate`, `node_pack` and the body bound.

## S3 request budget

None: pure encoding.

## Risks

- The writer is checked against an encoder written from the same reading of the Java; the R7 differential suite against a shell-written bucket is the first independent check.
- A whole manifest and its body are in memory at once during an edit: at most 1,024 commits and the 64 MiB body cap.

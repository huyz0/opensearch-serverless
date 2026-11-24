# Node pack format writers

## Serves

Parity plan R11 (node packs), invariant "one immutable blob per segment" (a pack holds whole segment blobs). First piece: the bytes, before any writer uses them; the pack store and writer, the publisher's placement and the collector follow as their own specs.

## Change

- `oss-format` gains `encode_node_pack`, ported from `NodePackBlob.encode` (version 2): the prefix and header of a pack for a list of planned blocks, and each block's offset. Offsets count the header, whose length does not depend on their values (fixed width), so the header is written once and the offsets patched in.
- `oss-format` gains `NodePackLedger` and `encode_pack_ledger`, ported from `NodePackLedger.encode`/`decode`: the per-node register listing each epoch, its collected prefix and its highest recorded ordinal.
- Not changed: the readers, which already read both; pack version 1 is never written.

## Acceptance criteria

- AC-1: the pack header the Java wrote (`fixtures/shell-blobs`) is re-encoded byte for byte from its parsed block.
- AC-2: a header of several blocks (non-ASCII and NUL in strings, an empty block) parses back to what was planned, and the offsets are consecutive from the end of the header.
- AC-3: a pack with no block, an empty uuid or name, a number past `i64`, a string past 65,535 bytes or a header past the limit is refused.
- AC-4: a ledger encodes to the Java's layout and parses back; an empty ledger is five bytes.
- AC-5: a ledger that is empty, of another version, short, with a negative count or number, with a count its bytes cannot hold, or with bytes left over is refused.

## Test plan

`crates/format/tests/pack_writers.rs`, one test per criterion. Written first and seen to fail (the module did not compile).

## S3 request budget

Unchanged: pure encoding, no I/O.

## Switch

None: nothing uses it yet. Node packs themselves arrive behind `segment_placement` with the publisher.

## Risks

A header the Java cannot open: AC-1 pins the bytes to the Java's. The exit gate for R11 opens a Rust-written pack with a Java reader.

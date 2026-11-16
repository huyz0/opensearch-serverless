# Node log format: write-log records and node-log blobs, byte for byte

## Serves

Parity plan R5.1, the first piece of the writer's node write log (roadmap stage 3, [roadmap](../../roadmap.md)). Invariant: one write log per node, never per shard ([principles](../../principles.md#write-path)). A mixed fleet takes over each other's logs only if the records are bit-identical.

## Change

- `oss-format` gains `WalRecord` (`wal_record.rs`), a port of the shell's `nodelog/WalRecord.java`: `toBytes` and `fromStream`, the accessors, and `EncodedWalRecord`, a record written once so a blob copies it rather than serialising it twice.
- `oss-format` gains `node_log` (`node_log.rs`), a port of `nodelog/NodeLogBlob.java`: `encode` (one window's records of every shard as one blob, with the running index), `encode_fence`, `header_end`, `decode` (a slot's header or fence), `slice_records` (one shard's records, one at a time), `with_ordinal`, `trim_below`; and `node_log_key` (`NodeLogStore.name` and `epochPath`: `nodelog/<node>/e=<epoch:020>/<ordinal:020>`).
- `mutf8` gains the `writeUTF` encoder the header's strings need.
- Not changed: no store I/O, no committer, no fencing or replay (R5.2 to R5.4). The node-log epoch a lease names (`NodeLogEpoch`) was ported with the lease in R4.1.
- Deliberately stricter than the Java on reading, where the Java's own writer never produces the input: a record's known field of a coerced type, `null`, a repeated field, or bytes after the object; a header with a negative number, a trim point past its ordinal, overlapping slices or runs, a shard indexed twice, or bytes left over. Listed in the module docs and `fixtures/node-log/records-strict.txt`.

## Acceptance criteria

- AC-1: for every record in `fixtures/node-log/records.txt`, `WalRecord::parse` accepts exactly what the shell's `fromStream` accepts and `write` of it is byte-identical to the Java's `toBytes` (`records-parsed.txt`); every line of `records-strict.txt` is refused.
- AC-2: for every case in `blob-cases.txt`, `encode` (or `encode_fence`) produces the bytes the shell's `NodeLogBlob.encode` wrote (`blobs.txt`), and refuses exactly the cases the Java refuses; `decode` of each Java blob reads what the Java's `decode` read (`decoded.txt`), and every slice parses back to the records its batch held.
- AC-3: `header_end` asks for more bytes below the prefix and gives the header's end from it on; `decode` of every cut of a header is `Truncated` with the length needed; every flipped header byte decodes or is refused without a panic; each strictness rule above is refused by its own test.
- AC-4: an encoding the Java refuses writes nothing: a string past 65,535 bytes of modified UTF-8, an epoch or ordinal past `i64::MAX`, a shard past `i32::MAX`, a trim point past the ordinal, a shard in two batches or whose prior runs reach this ordinal.
- AC-5: `encode` allocates the blob once, at its exact length, and copies each record's bytes once (`skip_copies_nothing`-style counting allocator test).

## Test plan

- AC-1, AC-2: `crates/format/tests/node_log.rs` over `fixtures/node-log/`, recorded by the shell's own `WalRecord` and `NodeLogBlob` at `429b4cbc` against the fork's own x-content and Jackson 3 (`NodeLogs.java`). Fails if a field is written in another order or escape, a name is placed differently, or the index is in UTF-8 rather than UTF-16 order (case `n4`).
- AC-3, AC-4: the same file, on raw headers built field by field.
- AC-5: `encode_allocates_the_blob_once` in `crates/format/tests/node_log_alloc.rs`, its own binary for the process-wide counting allocator.

## S3 request budget

Unchanged: pure encoding, no requests. The format is what makes R5.2's budget possible: one PUT per window for every shard on the node.

## Switch

None: a format port with nothing to compare against; the Java's own format is the only one.

## Risks

- A byte that differs from the Java's breaks takeover between Rust and Java nodes. AC-2 covers it per field; R5's exit gate replays logs across languages on the fs store.
- Strictness: a header Rust refuses is a shard a Rust successor cannot take over. Every rule refuses only what the Java's writer never writes.

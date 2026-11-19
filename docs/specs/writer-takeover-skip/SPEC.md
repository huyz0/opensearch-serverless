# Takeover reads only the log past the commit, and replays in bounded memory

## Serves

R8.1 follow-up to [writer takeover](../writer-takeover/SPEC.md), and the request-cost goal: requests scale with nodes and bytes, so a takeover must not fetch log blobs whose records for the shard a published commit already holds. Parity with the Java, which skips a record the restored commit holds only after reading it; this skips the read.

## Change

- `NodeLogTakeover::for_each_record_above(shard, through, each)`: as `for_each_record`, leaving out every slice whose highest sequence number for the shard is known and at or below `through`. A skipped slice is neither fetched nor parsed, and a blob all of whose slices for the shard are skipped costs no body or slice read, only the header the takeover reads anyway.
- `takeover::adopt` reads through it with the commit's `max_seq_no`, so only blobs holding records past the commit are fetched.
- `takeover::Engine` gains `buffered_bytes` and `flush`, and `adopt` takes a `max_buffer_bytes`: whenever the engine's buffered bytes (the writer's buffer and the version map) reach it, `adopt` flushes (commits locally, which releases the buffer and prunes the version map) before applying the next record, so a replay of any length holds at most the bound and one record. The one publish at the end is unchanged. The caller passes the shard's share of the node's indexing memory.
- Not changed, by design and listed: the manifest and the blob formats (no log position is added: a slice's own `max_seq_no` is the position), `for_each_record` and `for_each_record_after` (unchanged for their callers), records with no sequence identity (a slice with none is never skipped: its highest sequence number is unknown), filling sequence gaps with no-ops.

## Acceptance criteria

- AC-1: `for_each_record_above` hands over only records of slices whose highest sequence number for the shard is above `through`, oldest first; a slice with any record above is handed over whole, and the engine's own check drops the records inside it that the commit holds.
- AC-2: a blob whose slices for the shard are all at or below `through` costs no GET beyond the header reads, however large its body; the same replay through `for_each_record` reads it.
- AC-3: a slice holding only unsequenced records is never skipped; a slice mixing them with sequenced ones is judged by the sequenced ones (the node log's writers always assign identity).
- AC-5: `adopt` flushes the engine when its buffered bytes have grown by `max_buffer_bytes` (at least 1) past what the last flush left, before the next record, and not otherwise; a flush that leaves more than the bound does not cause one before every record.
- AC-6: a real shard replaying 200 records under a bound of a few records' bytes ends with every record applied, one publish, and a peak buffer at most the bound plus one record's cost.
- AC-4: `adopt` over a commit that holds everything the predecessor logged reads no slice and no body, and publishes nothing.

## Test plan

AC-1 to AC-3: `crates/writer/tests/node_log_replay.rs` additions over the emulator meter, each seen failing against a `for_each_record_above` that skips nothing. AC-4: `crates/writer/tests/takeover.rs` with the meter. AC-5: a recording engine. AC-6: a real shard, its `indexing_buffer_bytes` read between records by an engine wrapper.

## S3 request budget

Per shard taken: log GETs for the blobs holding records past the commit, not for every blob holding the shard's unpublished records; a takeover of a shard the commit fully covers reads headers only. 0 PUT added, 0 LIST.

## Switch

None: the skip loses nothing, since a skipped slice holds only operations the commit has.

## Risks

- Skipping a record the commit lacks: a slice is skipped only when its highest sequence number is at or below the commit's `max_seq_no`, which the writer assigns in order and commits only after applying all lower ones (AC-1, AC-3).

- `adopt` and its bound are not yet called from node assembly; that wiring belongs to the node-assembly work.

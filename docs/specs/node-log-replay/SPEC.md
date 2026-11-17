# Node log replay, part 1: read a shard's records from a fenced log

## Serves

Parity plan R5.4 (roadmap stage 3, [roadmap](../../roadmap.md)): a shard taken from another node, or reopened by its own, replays the records its writer logged and did not publish. Requests scale with nodes and bytes, never with records; nothing lists ([principles](../../principles.md#write-path), `s3-cost` skill).

## Change

- `NodeLogTakeover` gains the reading half of the shell's `nodelog/NodeLogTakeover.java` at `429b4cbc` (`crates/writer/src/log/takeover/read.rs`): `for_each_record`, `for_each_record_after`, `for_each_record_in`, `max_seq_no_of`, `holds`, and the header and body caches.
  - **By shard, from the index.** Each epoch's last header below the fence names the runs of slots holding each shard's unpublished records; a shard's records are read run by run, oldest epoch first, in the order they replay.
  - **Headers kept.** A blob's header is read once (one ranged GET) and kept for the other shards in the same blob.
  - **Bodies kept.** A blob holding several shards' slices is read whole once (one ranged GET) and kept for the others, within 8 MiB a body and 64 MiB a reader, for 60 s from the first (`BodyCache`, overridable per reader); past those bounds, or when the read failed, each shard reads its own slices. A blob of one slice is read by slice.
  - Records stream to a consumer, which may stop the read with its own error; nothing collects a shard's records whole.
  - A slot the index names that holds nothing (an append of unknown outcome that did not land) is skipped; one holding a fence is a corrupt log.
- Not changed: recovery into a shard (sequence-number skips, logged commits), `NodeLogWriter`'s own-log replay wiring.

## Acceptance criteria

- AC-1: across two epochs (the first fenced under the writer), a successor reads shard A's records `a0, a1, a2` in order, B's `b0, b1` (epoch 0's last header predates B's publish), and none of C's; `max_seq_no_of` is 2 for A and `UNASSIGNED_SEQ_NO` for C; `holds` is true for A and false for C.
- AC-2: three shards whose records share one blob: after the fence, the first shard's read takes GETs, and the other two read nothing more.
- AC-3: `for_each_record_after` a position leaves out what landed in that blob and before, including all earlier epochs; `for_each_record_in` reads known runs of one epoch and writes nothing.
- AC-4: a consumer's error stops the read and is returned as given; a failed body read fails that read, keeps nothing, and the next read succeeds.
- AC-5: a blob of one slice keeps no body; a shared body exactly at `BodyCache::max_body` is kept and one byte over is not, its shards then reading a slice each; a second body that would pass `max_kept` is not kept, and two that exactly fill it are; bodies kept past `keep_for` are let go and read again.
- AC-6: a log the shell's own `NodeLogCommitter` wrote on a filesystem store, across two epochs (the first closed by a foreign fence) with one blob three shards share, is fenced and replayed by the Rust takeover on `FsStore` with the same `ends`, the same `max_seq_no_of` and `holds` for each shard, every record byte for byte as the shell's `NodeLogTakeover` read it, and the same fence blob at the same key (`fixtures/node-log-replay/`). The Rust `NodeLogCommitter`, given the same calls, writes the same blobs byte for byte at the same keys and reports the same lease, so the shell replays a log the Rust writer wrote as it replays its own.

## Test plan

AC-6 replays `fixtures/node-log-replay/`, recorded by `NodeLogReplay.java` there (its README says how), on a copy of the recorded tree.

`crates/writer/tests/node_log_replay.rs` on the emulator, its meter counting `Put` and `Get`; the writer is a `NodeLogCommitter`. Ported code with its tests written alongside: `scripts/mutants.sh` on the diff stands in for watching each test fail.

## S3 request budget

- A shard's replay: per blob holding its unpublished records, one ranged GET for the header (shared with the other shards in that blob) and one for each of its slices, or one for the whole body shared by every shard in it. No LIST, no write.
- `holds` and the last headers: nothing past the fence's reads. `max_seq_no_of`: headers only.

## Switch

None: this ports behaviour the Java runs unswitched.

## Risks

- Unbounded memory from bodies kept: bounded by size, total and time (8 MiB, 64 MiB, 60 s), and a consumer that streams.
- A record replayed out of order: runs are in slot order and epochs in lease order (AC-1).

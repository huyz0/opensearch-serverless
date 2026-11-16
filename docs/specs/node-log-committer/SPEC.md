# Node log committer: group commit across every shard of a node

## Serves

Parity plan R5.2, part 2 (roadmap stage 3, [roadmap](../../roadmap.md)). Invariant I-5: a write is acknowledged after exactly one PUT to its node's write log, whatever the number of shards it touches ([invariants](../../invariants.md)). One write log per node, never per shard ([principles](../../principles.md#write-path)).

## Change

- `oss-writer` gains `log::NodeLogCommitter` (`crates/writer/src/log/committer/*`, `window.rs`, `epochs.rs`, `hedge_delay.rs`), a port of the shell's `nodelog/NodeLogCommitter.java`, `NodeLogWindow`, `NodeLogEpochState` and `NodeLogHedgeDelay` at `429b4cbc`:
  - **The window.** The first record to arrive opens a window; every record, of any shard, that arrives before it closes rides the same blob. A window closes when its time is up or it is full (bytes of id and source, or records); records past a window's bounds go on into the next, in order. `commit` returns once the blob holding the caller's records has landed, with where it landed.
  - **One blob in flight.** Windows are written in order, one at a time, by the writer that first joined the window, while later writers wait for it without the lock.
  - **Epochs.** Before the first blob of an epoch, the epoch is recorded through `EpochLease`. A failed append ends the epoch, whatever the failure; the next window opens a new one.
  - **Ownership.** A shard's first records in an epoch wait for `ShardAdmission` to confirm this node still owns it at that term, after the epoch is in the lease.
  - **Trimming.** Per epoch, the runs of slots holding each shard's unpublished records: `on_published` drops them, `positions` lists the epochs a successor must fence (for the lease), `take_complete` hands back epochs to delete, `trim_point` the lowest slot still needed.
  - **Hedging.** A window's PUT is sent again after the hedge delay: the setting, raised to the 95th percentile of the last 128 PUTs and scaled by the blob's size against their median, so at most about one PUT in twenty is sent twice.
  - `enqueue` and `flush_queued` (records nothing waits on, riding the next blob), `last_position`, `unpublished_runs`, `forget`, and the idle flush (`flush_when_idle`), off by default as in the Java.
- `log::settings`: `serverless.wal.node_log.{window_millis, max_bytes, max_records, hedge_after_millis, idle_flush}`, with the Java's defaults (250 ms, 8 MiB, 50,000, 60 ms, false). Registered with the node when the writer is wired.
- Differences from the Java, by design: the committer takes records as `WalRecord`s and encodes each once, on the caller's thread, before taking the lock. Admission checks run one after another; the Java's concurrent admissions (`admission_concurrency`, default 32) and the shard pins a split takes (`NodeLogPins`) come in a later piece. The per-phase timings come with the write-ack meter (R5.5); this keeps the blob, record and hedge counts.
- Not changed: no write fences on shards or terms (R5.3), no replay (R5.4), no wiring into a node.

## Acceptance criteria

- AC-1 (I-5): writers on 3, 30 and 300 shards committing at once within one window land in exactly one PUT, every caller gets the same position, and the blob's slices read back each shard's records (`a_window_of_any_number_of_shards_is_one_put`).
- AC-2: windows are written in order, one at a time, ordinals without gaps; a window full by records or bytes is written without waiting out its time, and a shard's records past a window's bounds continue, in order, in the next (`windows_close_when_full_and_spill_in_order`).
- AC-3: the first blob records a new epoch in the lease first; a lease failure fails the window and logs nothing; a fenced append fails every caller in its window with `Fenced`, ends the epoch, and the next window opens a new epoch after recording it (`a_fence_ends_the_epoch_and_the_next_window_opens_another`).
- AC-4: a shard's first records in an epoch are admitted once per (epoch, term); a refused shard fails alone while the rest of its window lands; `forget` makes it be checked again (`admission_is_checked_once_an_epoch_and_a_refusal_fails_only_its_shard`).
- AC-5: `on_published` moves the trim point up, `positions` lists the epoch being written and every earlier one still holding unpublished records, an epoch no longer written and fully published is handed back by `take_complete` once; an append whose outcome is unknown counts its records as unpublished (`trimming_follows_publishes_across_epochs`).
- AC-6: the hedge delay is the setting until PUTs are seen, then at least their 95th percentile, scaled up for a blob larger than their median; zero turns hedging off (`hedge_delay_*`, unit).
- AC-7: an enqueued record rides the next commit's blob without a PUT of its own and is told where it landed; `flush_queued` writes one blob when nothing else does, none when nothing is queued (`enqueued_records_ride_the_next_blob`).
- AC-8: with idle flush, a record finding the log quiet for a window is written at once; without it, it waits its window (`idle_flush_writes_a_lone_record_at_once`).

## Test plan

AC-1 to AC-5, AC-7, AC-8: `crates/writer/tests/node_log_committer.rs` and `node_log_committer_track.rs` on the emulator, its meter counting PUTs; the lease and admission are recording fakes. AC-6: unit tests in `hedge_delay.rs`. Ported code with its tests written alongside: `scripts/mutants.sh` on the diff stands in for watching each test fail, by showing a test fails for each plausible change (a PUT per caller, a skipped lease write, an admission every time).

## S3 request budget

- A window: 1 PUT, whatever its shards and callers (I-5); a second PUT only when hedged, at most about 1 in 20 by the 95th-percentile floor. The first window of an epoch adds the lease write, which the lease owner (R4) prices; admission adds what `ShardAdmission` reads, once per shard per epoch.
- `enqueue`: nothing of its own. `flush_queued`: 1 PUT when something is queued and nothing else wrote it.
- No GET or LIST on the write path; deletes of complete epochs are the caller's (`NodeLogStore::delete_range`).

## Switch

The idle flush is a setting, off by default as in the Java: it costs up to about half as many blobs again at one record a window. The rest ports behaviour the Java runs unswitched, with its defaults.

## Risks

- A write acknowledged before its blob landed, or a caller told it failed for a blob that then landed. Every caller waits on its window's result, set once after the PUT's outcome; a fenced or unknown outcome fails every caller in the window (AC-3).
- A slot skipped: ordinals advance only on a landed blob; a failed one ends the epoch (AC-2, AC-3).
- Throughput: sequential admission makes a window carrying thousands of shards new to an epoch wait one check per shard; concurrent admission is the next piece, before the committer is wired into a node.

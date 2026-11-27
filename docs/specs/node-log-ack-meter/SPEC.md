# Node log write-ack meter: where an acknowledgement's time goes

## Serves

Parity plan R5.5 (roadmap stage 3, [roadmap](../../roadmap.md)): the roadmap's write gate (write p99 within 5% of the shell) needs to know where a write's acknowledgement time goes before anything is tuned (`rust-performance` skill: measure first).

## Change

- `oss-writer` gains `log::AckTimings` and `NodeLogCommitter::{timings, reset_timings}` (`crates/writer/src/log/committer/timings.rs`), a port of the shell's `nodelog/NodeLogCommitTimings.java` at `429b4cbc`:
  - **Per caller:** the wait for its window to close, the wait behind the blob in flight before its own, and its own blob's write; counted when its window was written.
  - **Per blob:** recording an epoch (or finding one open), checking shards new to the epoch (and how many), and the PUT; and epochs opened. The hedge counts the committer already keeps ride along.
  - Sums since the last reset, read as means; `Display` is the Java's log line.
  - Lock-free sums (relaxed atomics); a window's result carries when it closed, started and ended writing.
- Not changed: no setting, no log line emitted on a timer (the node wiring's), no histogram.

## Acceptance criteria

- AC-1: one caller in a 50 ms window: 1 caller, 1 PUT, 1 epoch opened, 1 admission check, at least 45 ms waiting for its window, a nonzero PUT no longer than its write. A second commit of the same shard adds a caller and a PUT, and no epoch or admission.
- AC-2: 30 callers in one window: 30 callers, 1 PUT, 30 admission checks.
- AC-3: a caller joining while a 200 ms PUT is in flight counts at least 100 ms behind it.
- AC-4: a reset zeroes every sum; the line names callers, blobs, admission checks, hedges and epochs in the Java's format.
- AC-5: an open-load meter, off unless `OSS_ACK_SECONDS` is set, offers writes at random times at 100 a second over 200 shards, with a 250 ms window and a 20 ms PUT, as the shell's `WriteAckMeterTests` open mode does. It prints ack p50, p90 and p99 and the meter's line. Over 30 s in a release build its p50 is no more than 5% above the shell's 158 ms (166 ms). Provisional: emulator, one machine.

## Test plan

`crates/writer/tests/it/node_log_ack_meter.rs` on the emulator. Only lower bounds the committer's own waits guarantee are asserted, never an upper bound on time. `scripts/mutants.sh` on the diff.

## S3 request budget

None: no request is added.

## Switch

None: counting costs a few relaxed atomic adds a blob and a caller; no behaviour changes.

## Risks

- A caller counted twice or a blob not counted: AC-1, AC-2.

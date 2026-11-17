# Node log trimming: delete behind the lease

## Serves

Parity plan R5.2, part 3 (roadmap stage 3, [roadmap](../../roadmap.md)). One write log per node, never per shard ([principles](../../principles.md#write-path)); requests scale with nodes and bytes, and cleanup deletes by name with no LIST (`s3-cost` skill).

## Change

- `oss-writer` gains `log::LeaseKeeper` (`crates/writer/src/log/keeper.rs`), a port of the lease and trimming half of the shell's `node/NodeLogWriter.java` at `429b4cbc` (`renew`, `deleteBehind`, `withInherited`, `leaseLock`):
  - **One lock for the lease.** The keeper is the committer's `EpochLease`: recording a new epoch and a renewal each compute the epochs and write the lease under the keeper's lock, so a renewal cannot drop an epoch being recorded.
  - **Inherited epochs.** The epochs an earlier process of this node left in its lease are named first in every lease the keeper writes, and are never deleted here (the janitor's, later).
  - **Renew.** `renew(&committer)` takes the committer's complete epochs, writes the lease with the inherited epochs and `positions()`, then deletes behind that lease: in each epoch it names, the slots below its trim point not yet deleted; and each complete epoch whole, up to two slots past its landed hint (the slot an append of unknown outcome may have taken, and a fence).
  - **Deleted only behind a landed lease.** Nothing is deleted before the lease naming it trimmed, or leaving it out, has landed; a failed lease write deletes nothing and keeps the complete epochs for the next renewal. A failed delete is tried again at the next renewal from where it stopped; storage, never correctness.
- `NodeLogWriter`'s `LeaseWriter.write(epochs, opensEpoch)` becomes the trait `LeaseWrite`.
- Not changed: shard pins (`NodeLogPins`, `expirePins`), concurrent admissions, `flushQueued` at renewal (the caller's), the renewal timer and the node lease itself (R4's `oss-cluster`), replay (R5.4).

## Acceptance criteria

- AC-1: an epoch opened through the keeper writes one lease naming the inherited epochs then the new one, with `opens_epoch` true; a renewal writes one naming the inherited epochs then `positions()`, with it false.
- AC-2: after publishes move an epoch's trim point from 0 to 2, a renewal deletes slots 0 and 1 with one `Delete` request and no LIST, and the next renewal with no change deletes nothing and sends no request.
- AC-3: an epoch fenced and then fully published is left out of the renewal's lease and deleted whole, its fence included, in the same renewal, after the lease write: every slot of it is gone and the store holds no key under it.
- AC-4: a failed lease write deletes nothing, and the next renewal that lands deletes what the first would have. A failed delete is retried at the next renewal and then finishes.
- AC-5: inherited epochs are named in every lease and none of their slots is deleted.
- AC-6: a renewal racing new epochs being recorded never writes a lease that leaves out an epoch a blob has landed in, checked over many rounds from two threads.

## Test plan

`crates/writer/tests/node_log_keeper.rs` on the emulator, its meter counting `Delete` and `List`; the lease is a recording fake that can fail. Ported code with its tests written alongside: `scripts/mutants.sh` on the diff stands in for watching each test fail.

## S3 request budget

- A renewal: the lease write (R4 prices it) and at most one `Delete` request (free) a thousand slots trimmed; none when nothing moved. No GET, HEAD or LIST.
- Recording an epoch: the lease write, as before.

## Switch

None: this ports behaviour the Java runs unswitched, and deleting behind the lease only removes blobs nobody can read.

## Risks

- A slot deleted that a successor's fence would search from: deletes follow the landed lease whose trim point is above them (AC-2, AC-3).
- An epoch dropped from the lease while written: one lock across compute and write (AC-6).

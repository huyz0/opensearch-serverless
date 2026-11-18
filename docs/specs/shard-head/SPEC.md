# Shard head: the register that says who owns a shard

## Serves

Parity plan R7.6, second piece: the shard-head read the node writer's first write waits on (`write.first` in [request-budgets](../../request-budgets.md)), and the take that gives a node a shard to write. The head is the one object that arbitrates ownership ([principles](../../principles.md#write-path)).

## Change

- `oss-format` gains `ShardHead` (`crates/format/src/shard_head.rs`): the record the Java's `ShardHead.toBytes` writes and `fromStream` reads, byte for byte, with the Java's required fields (`index`, a non-negative `shard`, a positive `term`) and optional `owner_node_id`, `owner_ephemeral_id`, `lease_expires_at_millis`, `index_uuid` and `writers`. Unknown fields are skipped.
- `oss-cluster` gains `ShardHeadStore` (`crates/cluster/src/heads/`): `read` (one GET) and `acquire` (the Java's `ShardHeadStore.acquire` for an unbucketed shard): an absent head is created at term 1; a head left by another incarnation, or unowned, or whose owner is not live is taken at the next term, naming its writers; a live owner refuses with the winner's head. The key is `shards/<2 hex of murmur3(name) & 0xFF>-<index>#<shard>`. Liveness is the lease oracle's.
- `oss-node` gains `writer::HeadAdmission`, the committer's `ShardAdmission` over the head: one read, refusing unless the head names this node's process at the term; `writer::MembershipLeaseWrite`, the `LeaseKeeper`'s lease write through `Membership::renew`; and `writer::Incarnation`, the Java's `ensureIncarnationLive`: one descriptor read, none within a one-second fence window, and a shard whose index was replaced is released.
- The `write.first` rows of the request budget become enforced: 4 requests, depth 4 (a ceiling of 5).
- Not changed: bucket heads (shared shard heads), `renew`, `release`, `releaseAbandoned`, deleting heads; the delete lifecycle's `ShardHeads` trait stays unimplemented.

## Acceptance criteria

- AC-1: every case of `fixtures/shard-heads/cases.txt` reads and writes as the Java's `ShardHead` does (`parsed.txt`, written by the shell class itself), or is refused where the Java refuses.
- AC-2: truncated and corrupt heads are refused, never panic.
- AC-3: `read` of an absent head is one GET and answers none; of a head, one GET.
- AC-4: `acquire` of an absent head creates it at term 1 with one create request and wins.
- AC-5: `acquire` of a head whose owner is live refuses and writes nothing.
- AC-6: `acquire` of a head whose owner is gone takes it at term + 1 and names the old owner as the newest writer.
- AC-7: two nodes acquiring one absent head: exactly one wins and the other is told the winner.
- AC-9: the first write after a take is the incarnation read, the lease write, the head read and the log blob: 4 requests, depth 4, and the next write 1 and 1.
- AC-10: a writer reads the incarnation once inside the window, and refuses and releases a shard whose index was replaced.
- AC-8: admission passes for the owner's process at its term and refuses another node, another process, or another term.

## Test plan

`crates/format/tests/shard_heads.rs` over the recorded fixtures; `crates/cluster/tests/heads.rs` and `crates/node/tests/writer_heads.rs` on the emulator with its meter.

## S3 request budget

`read`: 1 GET. `acquire`: a GET and one create (absent head), or a GET and one swap, plus the oracle's lease read and revoke (read, swap) when the owner looks dead. No LIST.

## Switch

None: it ports the Java's ownership register, which the Java runs unswitched and Java nodes read.

## Risks

- A head written a byte differently from the Java: AC-1 over recordings from the shell's own class.
- Two owners: the create and swap are conditional, and the write log fences a paused writer.

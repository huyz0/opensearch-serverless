# 0066: A partial snapshot records the shards it could not capture, and a restore may bring it back

**Status:** Proposed

## Context

`partial=true` on a take means, here and in the Java shell, "skip an index that could not be captured in full": an index with a shard that published no commit is left out of the record. A finished record names every shard of every index it holds: the Java `SnapshotRecord.fromStream` refuses an index with some shards but not all (`a completed index must contain every shard`), and an index with a shard count and no shards is the in-progress marker (`SnapshotRecord.capturing`, `GarbageCollector.capturing`, and here `SnapshotIndex::capturing`). So a snapshot is never partial and a restore's `partial` has nothing to restore: both nodes refuse `partial: true` with 501. The Rust reader does not enforce the every-shard rule; it counts `shards` entries and refuses only a shard number past the count.

Stock OpenSearch means something else: a take with `partial=true` keeps a shard that failed, the snapshot ends `PARTIAL` and lists the failures, and a restore of it needs `partial=true` and brings back what was captured. The project owner has decided to lift the refusal in both implementations ([ADR 0065](0065-a-snapshot-captures-the-aliases-of-its-indices.md) did so for aliases). Doing it properly changes the shared record, which both nodes read, so this is the one decision for both.

What "failed" is here: a shard with no published commit when the take read its manifest. A shard that was created and never written to has none, and so does one whose first commit has not landed. The snapshot cannot tell them apart, and an unpublished log tail is outside every snapshot already, so the honest record is "no commit was captured for this shard". The restore of such a shard gives the shard back empty.

## Decision

### The format

An index entry of the snapshot record may carry one new field, written only when not empty:

```json
"failed_shards": [1, 3]
```

the shard numbers that had no commit, each below `number_of_shards`, ascending, no repeats. `shards` lists the captured shards only. A completed index is one whose `shards` and `failed_shards` together cover every shard exactly once. An index with neither is the in-progress marker. An index whose every shard failed has `failed_shards` of all of them and no `shards`.

**The marker predicate changes** to `shards.is_empty() && failed_shards.is_empty() && number_of_shards > 0`, at every call site: Rust `SnapshotIndex::capturing`, Java `SnapshotRecord.capturing`, `GarbageCollector.capturing` (`GarbageCollector.java`) and `IndexLifecycle.pinnedBy`, which uses it. A test for an all-failed index runs through the snapshot state a GET answers, the janitor's pins, an index delete and a restore, on both sides, before the writer exists.

A snapshot is partial when any index has `failed_shards`, derived from the record by shape, as in-progress is. The register's finished flag, the times and `shallow` do not change.

### Old readers, and the order things ship

A record with `failed_shards` is **not safe for any reader that has not been changed**, and the two kinds differ:

- An old **Java** node refuses the whole record loudly when an index has some shards and some failures (the every-shard rule), so it cannot list or restore that snapshot. For an all-failed index it sees the in-progress marker: the snapshot answers `IN_PROGRESS` forever and the janitor and index delete pin the index's blobs for ever.
- An old **Rust** node reads it without complaint: it counts the `shards` it finds, restores them without `partial` and answers success. That is silent loss.

So the order is fixed. (1) A Rust change, ahead of everything else, makes the restore's reader refuse an index whose distinct captured shards and failed shards do not cover every shard exactly once, unless it is the marker: 500 `snapshot_unreadable`. Every record written so far is complete, so it changes nothing today. (2) The Java reader, the predicate at its call sites and the take, and the Rust reader and the predicate. (3) Only then may the setting below be turned on. The setting is a node setting and cannot protect another cluster or an older node that shares the repository: flipping it on is an operator decision, taken when every node reading the repository has both changes, and the ADR does not claim the setting enforces it.

### The take

A setting, `serverless.snapshots.take.partial_shards` (`OSS_SNAPSHOTS_TAKE_PARTIAL_SHARDS`, the same key in Java, with a how-to entry), default off. Off, `partial=true` keeps today's meaning exactly: an index with a shard that published nothing is left out, and the existing take tests and `snapshot.take.requests` pass unchanged. On, with `partial=true`, an index with a shard that published nothing is kept and the shards without a commit are recorded in `failed_shards`; without `partial=true` a shard that published nothing still fails the take, 409 `nothing_published` as today. A take in which no index captured any shard is refused 409 `nothing_published` in both nodes (the Java currently finishes an empty record as a success when `partial` skipped everything: it is aligned to the Rust, with a conformance fixture), so an all-failed index appears only beside an index that captured a shard.

Cost: today a take stops reading an index's shards at the first one with no commit. With the setting on it reads the index's remaining manifests to learn which others failed: up to one GET for each remaining shard, bounded by the take's shard limit and the same read a complete index costs. A budget row for a partial take (k of n shards without a commit: n reads) is added, and the existing take rows are unchanged with the setting off.

### Reading a partial snapshot

The parsers keep the failed shard numbers (at most the shard count, 100 an index at the target scale and `MAX_SHARDS` as the bound) and validate them against the captured shard numbers with a bitset of `number_of_shards`: a number past the count, a repeat, or a shard both captured and failed is a corrupt record, and so is a gap or a repeated captured shard. This is strict everywhere, the summary included: a corrupt `failed_shards` changes what GET reports and what a restore does, so it is a corrupt record and not a field to skip (unlike the alias field). The janitor's pins stay lenient and pin the whole index by uuid, which is correct, because a failed shard owns no blobs.

`GET _snapshot/{repo}/{name}` and the list answer `"state":"PARTIAL"`, `"shards":{"total":N,"failed":F,"successful":N-F}` with N the index shard counts, and `failures` entries in stock's shape (`index`, `index_uuid`, `shard_id`, `reason` "no commit was published when the snapshot was taken", `status` `INTERNAL_SERVER_ERROR`; no `node_id`, as stock omits it when unknown), checked against the partial fixture in `crates/format/tests/stock_snapshot_info.rs`. `failures` holds at most 1,000 entries a snapshot with a limit test; `failed` is exact. For a complete snapshot `total` equals the captured shards as today.

### The restore

`partial` is a boolean on the restore, default false. `include_global_state` and `ignore_unavailable` stay as they are.

- An index with `failed_shards` and no `partial: true` is refused before any write, 500 `snapshot_restore_exception`, naming the index and its failed shards and saying to ask for `partial=true`. (Stock answers 500; kept so a client's handling does not change.)
- With `partial: true` the index is created whole: its descriptor has every shard, each captured shard is copied as today, and a failed shard gets no manifest. A reader opens a shard with no manifest as a never-written shard when its head is absent or another uuid's (`ShardFiles::open`), and the writer's first publish reads it as absent (`ShardPublisher::read_register`); a restored index has a fresh uuid. Nothing extra is written and nothing extra is read; tests pin a search over a restored failed shard (empty) and a first write to it.
- The shard bound counts the descriptor's shards (all N), not only the captured ones.
- A synchronous answer reports the failed shards in `shards` (`failed: F`, `successful: N-F`). Stock reports the failed shards allocated empty and `failed: 0`; reporting them is a deliberate **Differs**, so a client is never told a partial restore was complete. A background restore (ADR 0063) answers accepted and cannot carry it: it logs one line naming the failed shards and the index.
- `partial: true` on a snapshot with no failed shard changes nothing.

### Both implementations

The Rust node writes and reads the field and builds the take, the setting, the answers and the restore. The Java shell changes `SnapshotRecord` (the every-shard rule becomes cover-exactly-once, the field carried through `fromStream`, the constructors and `toBytes`, including the legacy adoption path), the predicate at the call sites above, the take and `SnapshotOperations`, the restore, `SnapshotRequestBodies` (`partial` leaves `RESTORE_REFUSED_KEYS`), and the snapshot-info answer (`writeSnapshotInfo` hard-codes no failures). Fixtures under `fixtures/snapshots/` gain, written by each side and read by the other: a partial record, an all-failed index beside a captured one, and the corrupt cases (a number past the count, a repeat, captured and failed together, a gap), with an oracle that does not run through the parser under test. A repo check plants a record with a new field and reads it with the previous reader's rules.

Docs and tables change with it: `partial` leaves `REFUSED` and `OFF` in `snapshot_restore_keys.rs`, restore-default-flags AC-2 and AC-4, `docs/compatibility.md` (the keys, the **Differs**), the snapshot-take, snapshot-restore and snapshot-info specs, the OpenAPI document, the budget table and the crates' READMEs.

## Out of scope

- Retrying or repairing a failed shard of a snapshot: the record states what was not captured.
- Standard (copying) snapshots: a restore of one is refused 501 here, so `partial` on it is moot; their failed shards would own no blobs.
- Stock-format repositories: a restore from one is unchanged, and the stock reader's refusal of a partial stock snapshot stays.
- `include_global_state`, which has its own ADR.

## Alternatives rejected

- **Keep skipping the whole index and call that partial:** the status quo; a restore's `partial` stays meaningless and the shards that did publish are lost.
- **Record a shard with no commit as captured and empty** (a shard entry with no files, no new field, so no old-reader hazard): hides that a shard had nothing, which is exactly what PARTIAL tells a client, and an empty manifest's readability is not established.
- **A record-level list of failures:** a restore of some indices would filter it; the marker rule is per index.
- **A state flag in the name register:** the finished flag is shared by every reader and the state is derivable from the record.
- **Writing the field without a switch, or on the setting's word alone:** an old Rust node would restore the survivors and report success; the Rust coverage check ships first and the setting is an operator's decision.
- **Writing an empty manifest for each failed shard on restore:** one PUT a shard for what a missing register already means, and it would change `never_activated`.

## Consequences

Both nodes can hold and restore a snapshot that is missing shards, and a synchronous answer says how many. A shard created and never written to now makes a `partial` take PARTIAL, where today it skips the index; that is the price of not telling a never-written shard from a failed one, and it only happens with the setting on. The Rust coverage check is a small change that stands on its own and ships first. Until every reader has both changes the setting stays off, and an operator, not the node, makes that call.

# 0066: A partial snapshot records the shards it could not capture, and a restore may bring it back

**Status:** Proposed

## Context

`partial=true` on a take means, here and in the Java shell, "skip an index that could not be captured in full": an index with a shard that published no commit is left out of the record, and a finished record names every shard of every index it holds (`SnapshotRecord.fromStream` refuses an index that has some shards but not all: `a completed index must contain every shard`; the in-progress marker is an index with a shard count and no shards). So a snapshot is never partial, and a restore's `partial` has nothing to restore: both nodes refuse `partial: true` with 501. Stock OpenSearch means something else: a take with `partial=true` keeps a shard that failed, the snapshot ends `PARTIAL` and lists the failures, and a restore of it needs `partial=true` and brings back what was captured.

The project owner has decided to lift the refusal in both implementations ([ADR 0065](0065-a-snapshot-captures-the-aliases-of-its-indices.md) did the same for aliases). Doing it properly changes the shared record, which both nodes read, so this is the one decision for both.

## Decision

### The format

An index entry of the snapshot record may carry one new field, written only when not empty:

```json
"failed_shards": [1, 3]
```

the numbers of the shards that had no commit to capture, each below `number_of_shards`, ascending, no repeats. `shards` lists the captured shards only. A completed index is one that has `shards` and `failed_shards` together covering every shard; an index with neither is still the in-progress marker as today. An index whose every shard failed has `failed_shards` of all of them and no `shards`, and is not in progress.

A record without the field reads as today. A reader that does not know the field and meets an index with some shards and some failures refuses the whole record loudly (the Java invariant above). That is deliberate, not silent, and it is why the writer is gated (below). The record's `shallow`, times and the register's finished flag do not change: a snapshot is partial when any index has `failed_shards`, derived from the record by shape, as `in progress` is.

### The take

A setting, `serverless.snapshots.take.partial_shards` (`OSS_SNAPSHOTS_TAKE_PARTIAL_SHARDS`, the same key in Java), **default off until the Java reader ships**. Off, `partial=true` keeps today's meaning, exactly: an index with a shard that published nothing is left out. On, with `partial=true`, the take keeps the index and records the shards that published nothing as `failed_shards`; without `partial=true` a shard that published nothing still fails the take, 400 as today. A take that captured no shard of any index is refused as today (`nothing published`). No new request: the manifest read that found no commit is the one that records it.

### Reading a partial snapshot

The summary parser counts the failed shards of an index without keeping the list (memory stays the size of the names). `GET _snapshot/{repo}/{name}` answers `"state":"PARTIAL"`, `"shards":{"total":N,"failed":F,"successful":N-F}` and one `failures` entry for each failed shard, `{"index":..,"index_uuid":..,"shard_id":..,"reason":"no commit was published when the snapshot was taken","node_id":null,"status":"INTERNAL_SERVER_ERROR"}`, as stock's shape. The list endpoint carries the same.

### The restore

`partial` is a boolean on the restore, default false, as in stock.

- A restore of an index that has `failed_shards` without `partial: true` is refused before any write, 500 `snapshot_restore_exception` naming the index and its failed shards and saying to ask for `partial=true`, as stock's `SnapshotRestoreException`. (Stock answers 500; the status is kept so a client's handling does not change.)
- With `partial: true` the index is created whole: its descriptor has every shard, each captured shard is copied as today, and a failed shard gets no manifest, which is how an index that has never had a write holds a shard (to be pinned by a test; if a reader refuses a shard with no manifest the restore writes an empty one, one request more for each failed shard).
- The answer's `shards` counts the failed ones (`"failed":F`, `"successful":N-F`) so data loss is never silent.
- `partial: true` on a snapshot with no failed shard changes nothing.

### Both implementations

The Rust node writes and reads the field, the setting, the answers and the restore. The Java shell changes `SnapshotRecord` (the invariant becomes `shards + failed_shards` cover every shard; the field is carried through `fromStream`, the constructors and `toBytes`, including the legacy adoption path), the take, the restore, `SnapshotRequestBodies` (`partial` leaves the refused keys) and the snapshot-info answer. Fixtures under `fixtures/snapshots/` gain a record with failed shards written by each side and read by the other, a record with an index whose every shard failed, and the corrupt cases (a number past the shard count, a repeat, a shard both captured and failed). Reading a corrupt `failed_shards` is a corrupt record for the restore and for the summary (which needs the count), so it is strict on both, unlike the alias field.

## Out of scope

- Retrying or repairing a failed shard of a snapshot: it is a record of what was not captured.
- A shard that captured but whose copy failed in a standard (copying) snapshot: those are not restored here.
- `include_global_state`, which has its own ADR.

## Alternatives rejected

- **Keep skipping the whole index and call that partial:** the status quo; it makes a restore's `partial` meaningless and loses the shards that did publish.
- **A record-level list of failures:** a restore of some indices would filter it, and the in-progress shape check stays per index; a field on the index entry is what a restore of that index needs.
- **A state flag in the name register:** the register's finished flag is shared by every reader and the state is derivable from the record by shape.
- **Writing the field without a switch:** an older Java node would refuse the whole snapshot loudly, including listing it; a setting keeps it off until the reader ships.

## Consequences

Both nodes can hold and restore a snapshot that is missing shards, and the answer says how many. Until the Java reader ships, the setting stays off and nothing is written that an older reader refuses. The cost of a restore does not change unless a reader needs an empty manifest. A partial snapshot restored with `partial=true` has fewer documents than the source: the client asked for it and is told.

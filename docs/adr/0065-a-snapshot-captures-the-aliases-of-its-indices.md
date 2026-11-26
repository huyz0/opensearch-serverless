# 0065: A snapshot captures the aliases of its indices, and a restore may bring them back

**Status:** Proposed

## Context

A shallow snapshot records each index's uuid, shard count, mapping, settings and every shard's commit ([ADR 0045](0045-shallow-snapshot-restore.md)); it records nothing of the aliases that point at the index. Stock OpenSearch captures them: a restore with `include_aliases` (default `true` in core) re-creates each alias of each restored index, `rename_alias_pattern` and `rename_alias_replacement` rename them, and `alias_write_index_policy` decides who keeps the write index when the alias already exists. This node and the Java shell both refuse those four keys with 501 (`RESTORE_REFUSED_KEYS`), and accept `include_aliases: false` only because it asks for what they do ([restore-default-flags](../specs/restore-default-flags/SPEC.md)). The two nodes take and restore each other's snapshots, so a record written by one is read by the other: the change is a change to the shared bytes and has to land in both.

The project owner has decided to lift the refusal in both implementations. `include_global_state` and `partial` are not part of this decision (see Out of scope).

## Decision

### The format

Each entry of `indices` in the snapshot record may carry one new field, written only when it is not empty:

```json
"aliases": { "<alias name>": { "is_write_index": true } }
```

The key is the alias name, a literal index-or-alias name as the alias routes accept. The value is an object with one optional boolean, `is_write_index`, true when this index was the alias's write index when the snapshot was taken and absent or false otherwise. No other member is written; a reader ignores members it does not know. The field is on the index entry, not the record, because an alias names an index set and a restore may take only some of the snapshot's indices.

The change is additive. A record without the field reads as an index with no aliases, so every record written so far restores as it does today. A reader that does not know the field skips it: the Rust reader skips unknown fields, and the Java `fromStream` reads the entry as a map and ignores what it does not name. The one hazard is a node that reads a record and writes it back: neither does (the take writes a provisional record and then the final one itself, and nothing rewrites another node's record), so no node drops the field from a record it did not write. Bounds: at most 64 aliases an index and an alias name at most 255 bytes; an index in more aliases is refused by the take (below), so the bound is never exceeded in a record.

### The take

The take already reads each captured index's descriptor. The descriptor's `aliased_by` list is a hint, not the truth ([alias-records](../specs/alias-records/SPEC.md)), so the take reads each named alias's own register once and records the alias only when the register still lists the index; `is_write_index` is true when that record's write member is the index. A data stream's backing index is not recorded as aliased by its stream (a stream is not an alias, and `include_aliases` does not restore streams).

Cost: one register read for each alias of each captured index, in the same round as the existing reads, so the round trips of a take do not grow. No LIST. An index in more than 64 aliases is refused by the take, 400, before any write. The take costs nothing extra for an index with no aliases.

### The restore

`include_aliases: true`, `rename_alias_pattern` and `rename_alias_replacement` are accepted; `alias_write_index_policy` stays refused. With `include_aliases` absent or `false` nothing changes (core's default is `true`; this node's stays `false`, a difference that stays in docs/compatibility.md rather than changing what every existing restore creates).

- Aliases are added after the restored index is unmarked, one alias mutation for each ([alias-mutations](../specs/alias-mutations/SPEC.md)), because an index that is still being restored refuses an alias change. An alias that does not exist is created; one that exists gains the restored index as a member.
- The restored index is the write index of an alias only when the snapshot said so and the alias did not exist before the restore, or existed with no write member. Otherwise it joins as a non-write member and the existing write index keeps the role. This is the safe reading of core's policy; it is not core's default.
- The alias name is renamed by `rename_alias_pattern` and `rename_alias_replacement` on the same terms as an index. An alias name that is already an index's or a data stream's name is refused before any write, 400 `invalid_alias_name_exception`, like an alias route does.
- The cost is one alias mutation for each alias of each restored index, bounded by the same 64, and is listed in the request-budget table as its own rows.
- A failure adding an alias happens after the indices exist and are unmarked. It is not rolled back: the answer is the alias error, naming the alias, and the restored indices stay (the restore's data is whole; the alias can be added by hand). This is a **Differs** against stock and is in the compatibility notes.
- With the background restore ([ADR 0063](0063-a-restore-may-run-in-the-background.md)) the thread adds the aliases after the last mark clears, and a failure there is logged and counted with the others.

### Both implementations

The Rust node writes and reads the field and builds the restore above. The Java shell changes `SnapshotRecord` (read and write the field), the shell's take (capture) and its restore (apply), and `SnapshotRequestBodies` (accept the keys), with the same rules. Records round-trip between them; the fixtures under `fixtures/snapshots/` gain a record with aliases written by each side and read by the other.

## Out of scope

- **`include_global_state`:** the global state is the cluster-wide records (index templates, ingest and search pipelines, stored scripts, settings). Each is a separate record family here with its own capture and restore, so it is its own decision and the keys stay refused.
- **`partial`:** a partial snapshot records shards that failed, which the record has no place for and which the Java reader refuses (`a completed index must contain every shard`). It needs a record shape (per-index failed shards), a snapshot state, the readers relaxed, and a restore that makes empty shards for what was missing. A separate ADR.
- A filter, routing or hidden flag on an alias: aliases here carry none ([alias-routes](../specs/alias-routes/SPEC.md)).

## Alternatives rejected

- **A record-level `aliases` map naming the indices of each alias** (core's metadata shape): a restore of some indices would have to filter it, and a stale entry for an index the snapshot did not capture would have to be ignored. A field on the index entry is exactly what a restore of that index needs.
- **Recording the descriptor's hint without confirming it:** saves one read for each alias, but a stale hint would bring back an alias someone removed. The confirm is bounded by the alias count.
- **Restoring aliases before the index is unmarked:** the alias mutation refuses a marked index, and marking is what keeps a half-restored index unreadable.
- **Rolling the restore back when an alias fails:** deletes the whole index for a bookkeeping failure and costs a delete of every shard's blobs.
- **Making `include_aliases` default true to match core:** changes what every existing restore request creates; a setting can flip it later.

## Consequences

A snapshot taken after this change carries aliases and a restore can bring them; a snapshot taken before restores as before. Both nodes must understand the field before either writes it, so the Java change lands first or with the Rust one (a Rust node reading a Java record, and the reverse, are covered by the fixtures). The take's cost grows by one read per alias of a captured index; the alias bound keeps it finite.

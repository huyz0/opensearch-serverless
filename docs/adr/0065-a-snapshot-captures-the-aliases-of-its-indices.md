# 0065: A snapshot captures the aliases of its indices, and a restore may bring them back

**Status:** Accepted

## Context

A shallow snapshot records each index's uuid, shard count, mapping, settings and every shard's commit ([ADR 0045](0045-shallow-snapshot-restore.md)); it records nothing of the aliases that point at the index. Stock OpenSearch captures them: a restore with `include_aliases` (default `true` in core) re-creates each alias of each restored index, `rename_alias_pattern` and `rename_alias_replacement` rename them, and `alias_write_index_policy` decides who keeps the write index when the alias already exists. This node and the Java shell both refuse those four keys with 501 (`RESTORE_REFUSED_KEYS`), and accept `include_aliases: false` only because it asks for what they do ([restore-default-flags](../specs/restore-default-flags/SPEC.md)). The two nodes take and restore each other's snapshots, so a record written by one is read by the other: the change is a change to the shared bytes and has to land in both.

The project owner has decided to lift the refusal in both implementations. `include_global_state` and `partial` are not part of this decision (see Out of scope).

## Decision

### The format

Each entry of `indices` in the snapshot record may carry two new optional fields, written only when they apply:

```json
"aliases": { "<alias name>": { "is_write_index": true } },
"aliases_truncated": true
```

The key is the alias name (1 to 255 bytes, a literal name as the alias routes accept). `is_write_index` is true when this index was the alias's *explicit* write member when the snapshot was taken, absent or false otherwise; an implicit write target (a single-member alias with no flag) is recorded as absent. `aliases_truncated` is true when the index was in more than 64 aliases and only the first 64 by name order were recorded. No other member is written; a reader ignores members it does not know. The fields are on the index entry, not the record, because a restore may take only some of the snapshot's indices.

The change is additive. A record without the fields reads as an index with no aliases, so every record written so far restores as it does today. An old reader skips the fields: the Rust `read_indices` skips unknown fields and Java `fromStream` reads the entry as a map and ignores what it does not name. Rewrites: the Rust take writes its own provisional and final record and nothing rewrites another node's. In Java, `SnapshotStore` re-encodes through `withStorageGeneration` and `toBytes`, and adopts a legacy record through `fromStream` then `encode`; the Java change must carry both fields through `fromStream`, the constructors and `toBytes`, and a test pins that the adoption path keeps them. Rust's stock export (`stock_export_route`) builds its own draft and does not carry aliases: stock repositories stay without them (Out of scope).

Read strictness. A summary parse (list, get, delete, the janitor's pins) never validates the alias fields, so a corrupt alias entry cannot break listing, pins or adoption. In Rust the summary parser skips them and only `RestoreIndex` reads them; in Java `fromStream` keeps the entry as an opaque unvalidated structure and carries it through `toBytes`, and only the restore path validates. The restore's reader treats a malformed field as a corrupt record, 500 `snapshot_unreadable`: `aliases` not an object, `is_write_index` or `aliases_truncated` not a boolean, more than 64 entries, an empty or over-255-byte name, a duplicate key. Fixtures cover each, and a corrupt alias entry that listing and adoption must still read.

### The take

A node setting, `serverless.snapshots.take.capture_aliases` (`OSS_SNAPSHOTS_TAKE_CAPTURE_ALIASES`, the same key in Java), alongside the take's other boot settings, with a how-to entry. **It defaults to `false`.** Off, a take is byte for byte what it writes today and keeps its exact request count (`snapshot.take.requests = 11`); the existing tests pin that and new tests cover on, both run in CI. The default flips to `true` in a follow-up change that names the measured budget rows, once the Java reader ships; that flip is an operator and release decision, not enforced, since a node-level setting cannot protect another cluster or an older Java node that shares the repository. With it on, an old Java node restoring a Rust record ignores the fields and still refuses the keys. With it off, a restore with `include_aliases: true` of a snapshot with no recorded aliases restores none and answers 200 with `"aliases_restored": 0`, so a client sees it.

On, after the shard commits, the take reads each captured index's descriptor (as today), collects the `aliased_by` hint names, and reads **each distinct alias once** (an alias shared by 100 indices is one read), with bounded fan-out (16 reads in flight). The hint is not the truth, so an alias is recorded for an index only when the register still lists it; the alias-mutations protocol prepares the descriptor hint before publishing the register, so a missing hint with a listing register cannot occur. The read is the plain resolve (a creating alias reads as absent, a pending update as itself), never the mutation resolve that waits. A data stream's backing index is recognised by the stream record (an alias read that returns a stream is skipped) and is not recorded. Bounds: the take sorts an index's hint names and, when the hint has more than 64, sets `aliases_truncated` and reads only the first 64 (some may then drop as stale); never a refusal and at most 6,400 distinct alias reads in a take (100 indices times 64). A restore of an index with `aliases_truncated` applies what is recorded, logs one line, and answers `"aliases_truncated": ["<index>", ...]`.

Cost: the alias reads depend on the descriptor reads, so a take with aliases is one round longer; a take is not the ranking path, and a take of indices with no aliases adds none. No LIST. Aliases are point in time per alias read, not per snapshot: an alias changed during the take may or may not be in the record.

### The restore

`include_aliases` (a boolean; non-boolean stays 400), `rename_alias_pattern` and `rename_alias_replacement` are accepted; `alias_write_index_policy` and the other keys stay refused. Absent, `include_aliases` is `false` (core's default is `true`; this stays a **Differs**). The rename keys follow the both-halves rule of `rename_pattern` (either alone is ignored) and are ignored without `include_aliases: true`. The key table moves the three out of `REFUSED` (ten remain) and `OFF`, and into `KNOWN`; restore-default-flags AC-2 and AC-4 change with it.

**Checked before any write.** `plan` refuses, with no store read, an alias name that equals a restored target name. The renamed alias name is validated as an alias name (255 bytes, the alias-name rules). Before the first index is created the restore resolves each distinct alias name once, keeps the result as the mutation's `before` (so the mutation does not read it again), and refuses 400 `invalid_alias_name_exception` when it names an index or data stream. A race after that check is a failure below.

**Applied after all indices.** The restore adds aliases once, after every restored index is unmarked, never after each index, so a later index's conflict rollback never leaves an alias member pointing at a deleted index. It makes **one alias mutation for each distinct alias**, adding every restored member in one update ([alias-mutations](../specs/alias-mutations/SPEC.md)). `plan` refuses before any write, in both modes, a request of more than 6,400 distinct aliases (the take's bound), 400; a limit test pins it. A foreground restore applies its mutations one after another inside the request's own time budget; one that cannot finish is the failure below for the aliases not yet added.

**Write index.** A restored member is the write index of an alias only when the snapshot said `is_write_index` and the alias has no write target at the moment of the mutation, explicit or implicit, by the same `AliasRecord` write-index semantics the routes use (an existing alias with one member and no flag still writes through that member, so the restored index joins as non-write). When two restored members both claim it, the first in plan order wins and the rest join as non-write. This is the safe reading of core's policy, not core's default.

**Failure.** One rule for both modes: an alias failure never deletes restored indices (their data is whole). A foreground restore answers 200 with the usual restore body plus `"failed_aliases": ["<alias>", ...]` (written in the OpenAPI document and the compatibility notes as a difference from every other restore failure on this node). A background restore has no per-restore result: as ADR 0063 and its AC-8 say for other failures, it writes one line to standard error and adds one to the failed count, so a client that never looks again does not learn of it. The alias phase in the background thread: cancel (AC-7) skips the aliases not yet added, logs, and keeps the indices; a panic in it (AC-5) must not run the guard that deletes the indices, since they are complete; and the mutation's buffers are part of the thread's reservation (AC-6). ADR 0063's rule that a failed thread deletes what it made still holds for failures of the copy, not once every index is complete. A retry of the restore is refused (the indices exist), so the aliases are added by hand with the alias API; this is a **Differs**.

### Both implementations

The Rust node writes and reads the fields, the setting and the restore above. The Java shell changes `SnapshotRecord` (both fields, the adoption path), the shell's take (capture, the setting), its restore (apply) and `SnapshotRequestBodies` (accept the keys), with the same rules. Fixtures under `fixtures/snapshots/` (the `GenTake`, `Check`, `rust-take`, `java-take` and `rust-restore` set) gain: a record with aliases written by each side and read by the other; `is_write_index` absent, false and true; truncated; and the corrupt cases above. Request-budget rows are added for a take with aliases (`d` reads for `d` distinct aliases, asserted exactly in the spec beside the constants it names) and for a restore with aliases (one resolve and one mutation for each distinct alias, the resolve shared). The compatibility notes, the restore-default-flags spec and the crates' READMEs change with it.

## Out of scope

- **`include_global_state`:** the global state is the cluster-wide records (index templates, ingest and search pipelines, stored scripts, settings). Each is a separate record family here with its own capture and restore, so it is its own decision and the keys stay refused.
- **`partial`:** a partial snapshot records shards that failed, which the record has no place for and which the Java reader refuses (`a completed index must contain every shard`). It needs a record shape (per-index failed shards), a snapshot state, the readers relaxed, and a restore that makes empty shards for what was missing. A separate ADR.
- A filter, routing or hidden flag on an alias: aliases here carry none ([alias-routes](../specs/alias-routes/SPEC.md)).

## Alternatives rejected

- **A record-level `aliases` map naming the indices of each alias** (core's metadata shape): a restore of some indices would have to filter it, and a stale entry for an index the snapshot did not capture would have to be ignored. A field on the index entry is exactly what a restore of that index needs.
- **Recording the descriptor's hint without confirming it:** saves one read for each alias, but a stale hint would bring back an alias someone removed. The confirm is bounded by the alias count.
- **Restoring aliases before the index is unmarked:** the alias mutation refuses a marked index, and marking is what keeps a half-restored index unreadable.
- **Rolling the restore back when an alias fails:** deletes the whole index for a bookkeeping failure and costs a delete of every shard's blobs.
- **Refusing a take of an index in more than 64 aliases:** a regression for a take that works today; a recorded truncation costs nothing.
- **One alias mutation for each (index, alias) pair:** N sequential mutations contending on one register; one for each distinct alias does it in one.
- **Making `include_aliases` default true to match core:** changes what every existing restore request creates; a setting can flip it later.

## Consequences

A snapshot taken with `snapshot.capture_aliases` on carries aliases and a restore with `include_aliases: true` can bring them; a snapshot taken before restores as before. The setting defaults to `false` until the Java reader ships, so no older Java node loses aliases silently; the flip to `true` is its own small change after the budget rows measure the take. A take with aliases is one round longer and reads one register for each distinct alias, bounded at 6,400.

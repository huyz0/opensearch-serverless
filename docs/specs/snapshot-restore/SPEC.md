# Snapshot restore, shallow

## Serves

[ADR 0045](../../adr/0045-shallow-snapshot-restore.md): `POST /_snapshot/{repo}/{name}/_restore` for a shallow snapshot a Rust or a Java node took, as the shell restores one.

## Acceptance criteria

- AC-1: A restore of a snapshot taken by Rust creates each captured index under its original name, or under `rename_pattern` and `rename_replacement`, with the captured mapping, settings and shard count, a new uuid, and each shard's files copied into the new index's first-term directory and its manifest landed; the restored index answers a search with what the snapshot captured. Test: a take, a restore and a search over the emulator.
- AC-2: The whole request is refused before any write when: the snapshot is missing (404 `snapshot_missing_exception`) or still being taken (409 `snapshot_in_progress`); a named index was not captured (404 `index_not_found`); two targets collide (400 `index_already_exists`); a target exists (400 `index_already_exists`); more than 100 indices or 4,096 shards (400 `snapshot_too_large`); a key the node does not serve is given (501), or an unknown one (400).
- AC-3: A failure after the first write (a store failing at each call, a manifest that cannot land because a writer published first, 409 `restore_conflict`) deletes every index this restore created and returns the first failure.
- AC-4: The copy streams through one bounded buffer: peak memory does not grow with the file's length. A request that cannot reserve the buffer is 429 and writes nothing.
- AC-5: Requests are one write a captured file and one ranged read for each buffer of its bytes (one for a file under the buffer), one manifest create a shard, one descriptor create an index, and the snapshot's read; no LIST, and the same for a 1 KB and a 100 MB file. A failed restore adds the index-delete service's requests for each index it created and no others. Budget rows `snapshot.restore_two.requests` and `snapshot.restore_cleanup.requests` (a restore that fails on its second shard, cleanup included), each spent by a gated test.
- AC-6: With any of the three switches off the restore answers 501 and makes no request; a snapshot that copied its data, or a record with a file of no recorded length, is refused 501 and written nothing.
- AC-7: A Java node reads an index a Rust node restored (recorded fixture and a Java-side test), and a Rust node restores a snapshot a Java node took (recorded fixture); the restore switch stays off until both pass.
- AC-8: A record whose settings carry another index's identity (uuid, creation date, version, provided name, shard count, replicas) or a shard mode's unit layout is refused 501 before any write, so only settings a restored index may keep are applied; a record with a layout or split is refused 501 `unsupported_snapshot_layout` before any write; a search over a restored index with rollups never rules out a restored document; a failure inside the descriptor create still deletes the index.

## Not yet done

The route ships behind its switch, off by default, with these still open, so this spec has no VERIFIED.md yet:

- The 403 `system_index` refusal: the node holds no registry of system indices to check a target against.
- AC-1's search over a restored index, and AC-7's recorded Java fixtures and Java-side read. Until both pass the switch stays off in every deployment.

## Out of scope

Standard snapshots, `include_global_state`, `partial`, `index_settings`, `ignore_index_settings`, `include_aliases`, and an asynchronous restore.

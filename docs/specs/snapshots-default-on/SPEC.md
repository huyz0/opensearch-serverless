# Snapshot reads and the take on by default

## Serves

[ADR 0038](../../adr/0038-snapshots-read-first.md): `serverless.snapshots.enabled` (`OSS_SNAPSHOTS_ENABLED`) defaults to true, so a node serves the snapshot and repository routes and a delete of an index reads the snapshots before it purges. `serverless.snapshots.take.enabled` (`OSS_SNAPSHOTS_TAKE_ENABLED`) defaults to true too, so a node takes, lists and deletes shallow snapshots. `serverless.snapshots.restore.enabled` (`OSS_SNAPSHOTS_RESTORE_ENABLED`) defaults to true as well, so a node restores a shallow snapshot a Rust or a Java node took.

## Change

The boot default of `OSS_SNAPSHOTS_ENABLED` is `true`. `assemble::Config::new` keeps `snapshots: false`, so the assembled-node tests that pin the old behaviour are unchanged; a node started from configuration is the one that changes.

Why it passes the switch rule ([principles](../../principles.md#switches-and-defaults)):

- No request is made unless one is asked for: the routes read on demand, and an idle node makes none.
- A Java shell takes snapshots (shallow ones name an index's blobs without copying them). A Rust node that does not read them purges blobs a snapshot names when it deletes the index. With the reads on, `StoreSnapshotPins` holds those shards back.
- The one cost is on an index delete: one LIST page of `snapshots/` and one GET for each snapshot, 11 requests for a delete that finds none, against 10 (budget row `index.delete_no_snapshots.requests`). A delete is an admin call, never on the idle or search path, and the count does not scale with documents or indexes.

## Acceptance criteria

- AC-1: a node started with no snapshot variable serves the read routes (`GET /_snapshot/{repo}` answers 404 `repository_missing_exception` for none) and the take (a take into a missing repository answers 404 `repository_missing_exception`), and the restore (a restore of a missing snapshot answers 404 `snapshot_missing_exception`). Test: `a_node_with_no_variable_serves_the_snapshot_reads_the_take_and_the_restore`.
- AC-2: `OSS_SNAPSHOTS_ENABLED=false` takes the routes away (the pre-change 400). Test: `the_variable_set_to_false_takes_the_snapshot_routes_away`.
- AC-3: the three switches read as set: all three on, each settable off alone; the take and the restore stay unserved unless `snapshots` is on as well. Test: `the_reads_the_take_and_the_restore_are_on_until_set_and_each_stands_alone`.
- AC-4: a delete that reads the snapshots and finds none costs 11 requests, one LIST page more than one that does not read them. Test: `a_delete_that_reads_no_snapshots_costs_one_list_more_than_one_that_does_not`.
- AC-5: with the take on in every Rust node, a Rust node takes a shallow snapshot of an index a Java node writes, deletes the index (the snapshot's blobs stay), a Java node describes the snapshot, restores it and finds what it captured, the Rust node, with the restore switched on, restores it too and finds the same, and the Rust node deletes the snapshot, which the Java node then reads as gone. Test: `a_snapshot_taken_beside_a_java_writer_survives_the_index_and_restores_on_java` (`crates/node/tests/mixed_fleet/snapshots.rs`, run by hand against RustFS and a live Java shell, as the mixed-fleet soak is).
- AC-6: the take's fault tests run as they always have, whatever the switch: the take releases its name, removes its marker as the ADR says, and leaves nothing that names storage, with the Nth store call failing at each point. Tests: the AC-4 list of [snapshot-take](../snapshot-take/SPEC.md).

## S3 request budget

Row `index.delete_no_snapshots.requests` (11, exact); `index.delete_pinned.requests` (12) is unchanged. A take costs what its rows say (11 requests for one index of one shard, `snapshot.take.requests`) and only when asked; an idle node makes none.

## Switch

The existing `serverless.snapshots.enabled` and `serverless.snapshots.take.enabled`, now on by default; an operator turns it off with `OSS_SNAPSHOTS_ENABLED=false`. Both modes stay tested: the assembled-node tests run it off, the boot tests on.

## Why the restore passes the switch rule

A restore's downside was that a node dying part-way left an index that answered from empty shards, with no error. The descriptor now carries a mark until the last shard lands ([snapshot-restore](../snapshot-restore/SPEC.md) AC-9), so a Rust node refuses that index (search 503, writes find none, take, point in time and alias refuse) and a delete clears it. What remains: one swap more an index restored, a rare admin request and not a read or write path, and a Java node in a mixed fleet: since huyz0/OpenSearch #184 it refuses a search and a write on a marked index as a Rust node does, though a get, a multi-get or a point in time on it is not covered. The restore's 429, bound and peak-allocation tests have run (`snapshot_restore_bounds.rs`, `restore_peak.rs`: 729 bytes beyond the two buffers for a 48 MiB file), and a Rust node restores a Java take and a Java node restores a Rust take. An operator turns it off with `OSS_SNAPSHOTS_RESTORE_ENABLED=false`; both modes stay tested.

## Out of scope

`serverless.system_indices.patterns` stays empty, as the Rust node loads no plugin that declares a system index; an operator who runs plugin indices on Java nodes names them.

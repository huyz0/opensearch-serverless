# Snapshot reads and the take on by default

## Serves

[ADR 0038](../../adr/0038-snapshots-read-first.md): `serverless.snapshots.enabled` (`OSS_SNAPSHOTS_ENABLED`) defaults to true, so a node serves the snapshot and repository routes and a delete of an index reads the snapshots before it purges. `serverless.snapshots.take.enabled` (`OSS_SNAPSHOTS_TAKE_ENABLED`) defaults to true too, so a node takes, lists and deletes shallow snapshots. `serverless.snapshots.restore.enabled` stays false.

## Change

The boot default of `OSS_SNAPSHOTS_ENABLED` is `true`. `assemble::Config::new` keeps `snapshots: false`, so the assembled-node tests that pin the old behaviour are unchanged; a node started from configuration is the one that changes.

Why it passes the switch rule ([principles](../../principles.md#switches-and-defaults)):

- No request is made unless one is asked for: the routes read on demand, and an idle node makes none.
- A Java shell takes snapshots (shallow ones name an index's blobs without copying them). A Rust node that does not read them purges blobs a snapshot names when it deletes the index. With the reads on, `StoreSnapshotPins` holds those shards back.
- The one cost is on an index delete: one LIST page of `snapshots/` and one GET for each snapshot, 11 requests for a delete that finds none, against 10 (budget row `index.delete_no_snapshots.requests`). A delete is an admin call, never on the idle or search path, and the count does not scale with documents or indexes.

## Acceptance criteria

- AC-1: a node started with no snapshot variable serves the read routes (`GET /_snapshot/{repo}` answers 404 `repository_missing_exception` for none) and the take (a take into a missing repository answers 404 `repository_missing_exception`), and answers 501 to a restore. Test: `a_node_with_no_variable_serves_the_snapshot_reads_and_the_take_but_not_the_restore`.
- AC-2: `OSS_SNAPSHOTS_ENABLED=false` takes the routes away (the pre-change 400). Test: `the_variable_set_to_false_takes_the_snapshot_routes_away`.
- AC-3: the three switches read as set: reads and take on, restore off, each settable alone; the take and the restore stay unserved unless `snapshots` is on as well. Test: `the_reads_and_the_take_are_on_and_restore_off_until_set_and_each_stands_alone`.
- AC-4: a delete that reads the snapshots and finds none costs 11 requests, one LIST page more than one that does not read them. Test: `a_delete_that_reads_no_snapshots_costs_one_list_more_than_one_that_does_not`.
- AC-5: with the take on in every Rust node, a Rust node takes a shallow snapshot of an index a Java node writes, deletes the index (the snapshot's blobs stay), a Java node describes the snapshot, restores it and finds what it captured, the Rust node, with the restore switched on, restores it too and finds the same, and the Rust node deletes the snapshot, which the Java node then reads as gone. Test: `a_snapshot_taken_beside_a_java_writer_survives_the_index_and_restores_on_java` (`crates/node/tests/mixed_fleet/snapshots.rs`, run by hand against RustFS and a live Java shell, as the mixed-fleet soak is).
- AC-6: the take's fault tests run as they always have, whatever the switch: the take releases its name, removes its marker as the ADR says, and leaves nothing that names storage, with the Nth store call failing at each point. Tests: the AC-4 list of [snapshot-take](../snapshot-take/SPEC.md).

## S3 request budget

Row `index.delete_no_snapshots.requests` (11, exact); `index.delete_pinned.requests` (12) is unchanged. A take costs what its rows say (11 requests for one index of one shard, `snapshot.take.requests`) and only when asked; an idle node makes none.

## Switch

The existing `serverless.snapshots.enabled` and `serverless.snapshots.take.enabled`, now on by default; an operator turns it off with `OSS_SNAPSHOTS_ENABLED=false`. Both modes stay tested: the assembled-node tests run it off, the boot tests on.

## Why the restore stays off

A restore creates the index's descriptor first and copies each shard after it, as the Java shell's restore does, so a node that dies in between leaves an index that is visible and searchable with some shards empty and no error to say so; only a delete clears it. These nodes are replaceable, so a crash is routine, and the restore is one request over up to 4,096 shards, so the window grows with bytes. That is a downside, so the restore stays behind its switch, off. Writing the descriptor last would break [ADR 0028](../../adr/0028-orphan-shard-sweep.md)'s rule that a shard container exists only for an index whose descriptor does. Open ways out, none built: a flag on the descriptor cleared by the last manifest create, which makes a half-restored index detectable and refusable, or a marker the janitor sweeps. The switch turns on when one of them exists with a crash test. Its 429 and bound tests have run (`snapshot_restore_bounds.rs`); peak allocation itself is not measured.

## Out of scope

`serverless.system_indices.patterns` stays empty, as the Rust node loads no plugin that declares a system index; an operator who runs plugin indices on Java nodes names them.

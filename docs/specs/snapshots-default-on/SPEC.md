# Snapshot reads on by default

## Serves

[ADR 0038](../../adr/0038-snapshots-read-first.md): `serverless.snapshots.enabled` (`OSS_SNAPSHOTS_ENABLED`) defaults to true, so a node serves the snapshot and repository routes and a delete of an index reads the snapshots before it purges. `serverless.snapshots.take.enabled` and `serverless.snapshots.restore.enabled` stay false.

## Change

The boot default of `OSS_SNAPSHOTS_ENABLED` is `true`. `assemble::Config::new` keeps `snapshots: false`, so the assembled-node tests that pin the old behaviour are unchanged; a node started from configuration is the one that changes.

Why it passes the switch rule ([principles](../../principles.md#switches-and-defaults)):

- No request is made unless one is asked for: the routes read on demand, and an idle node makes none.
- A Java shell takes snapshots (shallow ones name an index's blobs without copying them). A Rust node that does not read them purges blobs a snapshot names when it deletes the index. With the reads on, `StoreSnapshotPins` holds those shards back.
- The one cost is on an index delete: one LIST page of `snapshots/` and one GET for each snapshot, 11 requests for a delete that finds none, against 10 (budget row `index.delete_no_snapshots.requests`). A delete is an admin call, never on the idle or search path, and the count does not scale with documents or indexes.

## Acceptance criteria

- AC-1: a node started with no snapshot variable serves the read routes (`GET /_snapshot/{repo}` answers 404 `repository_missing_exception` for none) and answers 501 to a take and to a restore. Test: `a_node_with_no_variable_serves_the_snapshot_reads_but_neither_take_nor_restore`.
- AC-2: `OSS_SNAPSHOTS_ENABLED=false` takes the routes away (the pre-change 400). Test: `the_variable_set_to_false_takes_the_snapshot_routes_away`.
- AC-3: the three switches read as set: reads on, take and restore off, each settable alone; the take and the restore stay unserved unless `snapshots` is on as well. Test: `the_reads_are_on_and_take_and_restore_off_until_set_and_each_stands_alone`.
- AC-4: a delete that reads the snapshots and finds none costs 11 requests, one LIST page more than one that does not read them. Test: `a_delete_that_reads_no_snapshots_costs_one_list_more_than_one_that_does_not`.

## S3 request budget

Row `index.delete_no_snapshots.requests` (11, exact); `index.delete_pinned.requests` (12) is unchanged. Nothing else makes a request that did not before.

## Switch

The existing `serverless.snapshots.enabled`, now on by default; an operator turns it off with `OSS_SNAPSHOTS_ENABLED=false`. Both modes stay tested: the assembled-node tests run it off, the boot tests on.

## Out of scope

The take and the restore defaults. The take stays off until its fault tests and a mixed-fleet soak have run with it on; the restore stays off until a crash part-way is cleaned up and its 429 and peak-memory tests have run.

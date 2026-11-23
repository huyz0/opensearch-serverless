# The janitor deletes by default

## Serves

[ADR 0020](../../adr/0020-janitor-crate.md): "The default flips on evidence. Changing `serverless.janitor.dry_run` to off ... needs the fault suites (R8.6) and the mixed-fleet runs (R8.7) to show no live blob is ever counted, recorded in the spec's `VERIFIED.md`, in a PR of its own." This is that PR. The switch rule ([principles](../../principles.md#switches-and-defaults)) keeps the setting: deleting has a downside (a wrong deletion is unrecoverable), so the cluster default is on-delete only on the evidence below, and `OSS_JANITOR_DRY_RUN=true` goes back to counting on any node.

## Change

- `serverless.janitor.dry_run` defaults to false, in the setting, in the node's `assemble::Config`, and in the node's environment (`OSS_JANITOR_DRY_RUN`, default false).
- The orphan-shard sweep and the dead-log pass read the same node flag, so they delete too. The orphan-shard sweep still runs only with a slice budget above 0, which stays off by default.
- Nothing else changes: the three deletion rules (older term, unreferenced, two sightings and the minimum age) and the pins (manifest, points in time, snapshots) are as they were.

## Acceptance criteria

- AC-1: the setting's default and a node's default are live (not a dry run), and `OSS_JANITOR_DRY_RUN=true` makes a node a dry run that counts and deletes nothing.
- AC-2: over a writer that publishes repeatedly, and over a successor after a takeover, a sweep deletes no blob the commit names, and deletes a blob of an older term nothing names.
- AC-3: the per-shard sweep keeps every blob of an index incarnation a snapshot names, a point in time pins, or a Java-written commit names (the evidence of [janitor-snapshot-pins](../janitor-snapshot-pins/SPEC.md), [janitor-point-in-time-pins](../janitor-point-in-time-pins/SPEC.md) and [mixed-takeover](../mixed-takeover/SPEC.md)).
- AC-4: the proactive-takeover fault suites ([proactive-takeover-faults](../proactive-takeover-faults/SPEC.md)) pass with the live default, so a failed, raced or repeated take leaves nothing the janitor then deletes.

## Test plan

`crates/node/tests/janitor_live_evidence.rs` for AC-2; the existing suites named in AC-3 and AC-4 run unchanged against the new default; `crates/node/tests/server/assemble.rs` and the unit tests for AC-1, with the dry-run behaviour kept under its own test.

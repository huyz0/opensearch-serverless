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
- AC-4: after a take that fails, and after one whose answer was lost, a live sweep deletes no blob the commit names.
- AC-5: a shard asleep is not swept; once woken and published again a live sweep deletes only what no commit names.
- AC-6: a sweep between an acknowledged write and its publish deletes no blob the commit then names.
- AC-7: the dead-log pass, live, leaves a log still needed and collects it only once a successor has published all of it.

## What this evidence is not

The mixed-fleet requirement of the ADR (R8.7) was met as a recording: the Java side is the shell's own classes run once over the filesystem layout, and its commit is what the Rust sweep is held against ([mixed-takeover](../mixed-takeover/SPEC.md)); no Java node runs live beside a Rust one, and the emulator stands for the bucket. The setting is read from `OSS_JANITOR_DRY_RUN` once at boot, so going back to counting needs a restart of the node.

## Test plan

`crates/node/tests/janitor_live_evidence.rs` for AC-2, AC-5 and AC-6, `proactive_faults.rs` for AC-4; the existing suites named in AC-3 and AC-4 run unchanged against the new default; `crates/node/tests/server/assemble.rs` and the unit tests for AC-1, with the dry-run behaviour kept under its own test.

# A node removes the tombstones of deleted indexes

## Serves

Parity plan R8.4 (the janitor's tombstone sweep), invariant I-1 (every store call metered) and the fencing rule. `DescriptorStore::sweep_tombstones` ([tombstone-sweep](../tombstone-sweep/SPEC.md)) removes a tombstone once its quarantine has passed, but nothing in a node runs it: every deleted index leaves a register that each listing reads for good. `TombstonePass` is that run, ported from the shell's `reconcile` calling `TombstoneSweep` on its schedule.

## Change

`oss-node`, with the `writer` feature, gains `TombstonePass`, `TombstoneParts` and `TombstoneReport`, called by a booted node on the thread that already runs the dead-log pass.

- **One sweeper for the fleet.** Of the live members of the latest refresh, the one that ranks first by rendezvous on the fixed name `tombstone-sweep` (`reapers_for`, limit 1) sweeps; every other node makes no request. A death moves the role at the next refresh; two nodes that briefly disagree contend on the claim, which the sweep already arbitrates.
- **On a wait, not every pass.** The sweeper sweeps when `every` has passed since its last sweep, or since the pass was made, so a node that has just booted makes no request. The default is an hour (`OSS_TOMBSTONE_SWEEP_MS`, 0 turns the sweep off). A sweep is one LIST of the marker prefix while every marker is younger than the quarantine, and otherwise a page of markers an hour of deletes, so the fleet's cost is one LIST an hour and scales with deletes, never with indexes or nodes.
- **Conditional delete is proved, once.** The sweep removes registers only on a store whose conditional delete is proved ([`DeleteMode::probe`]); the sweeper runs the probe at its first due sweep and keeps a proved answer. A store that does not honour it, or a probe that failed, is never swept, and is probed again at the next interval (a few requests an interval on such a store, only on the sweeper).
- **Bounded.** The pass reserves one read buffer from the request-body pool before allocating; a pool without room skips the sweep and says so, and the wait is not reset.
- The quarantine is the Java's six hours (`DEFAULT_QUARANTINE_MILLIS`), the page the store's maximum.

## Acceptance criteria

- AC-1: a node that is not the sweeper makes no request, and reports it is not.
- AC-2: the sweeper does nothing before `every` has passed since the pass was made, and makes no request.
- AC-3: past `every`, the sweeper proves the conditional delete, then removes a tombstone whose quarantine has passed together with its marker, and keeps one still inside it.
- AC-4: the next sweep waits `every` again; before it, no request.
- AC-5: with nothing deleted, a sweep is one LIST and nothing else (after the probe, which is made once).
- AC-6: on a store that does not honour the conditional delete, tombstones and markers are all kept and the pass reports it did not sweep; it probes again at the next interval, and once the store honours it the sweep runs.
- AC-11: the probe runs only when a sweep is due on the designated node (not when off, not designated, not yet due), and once only after it proves the delete; its requests are a row of the request budgets.
- AC-7: a pass with no memory for its buffer sweeps nothing, reports why, and sweeps at the next call.
- AC-8: `every` of 0 turns the pass off: no probe, no request.
- AC-9: when the sweeper leaves the live members, the next live node that ranks first sweeps, and the one that left does not.
- AC-10: a booted node removes the old tombstone of a deleted index on its own, over a file store, with `OSS_TOMBSTONE_SWEEP_MS` short; unset it is an hour and `0` is off.

## Test plan

`crates/node/tests/roles/tombstone_pass.rs` over the emulator and a fake clock; AC-10 through `boot::start` over a file store, the tombstone stamped in 2020, and the config test for the variable.

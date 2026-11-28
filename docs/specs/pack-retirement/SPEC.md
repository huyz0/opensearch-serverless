# Retiring a departed Java node's packs

## Serves

[pack-collector](../pack-collector/SPEC.md) AC-9, the last part of slice 4 ([ADR 0025](../../adr/0025-node-pack-collection.md) decision 1, option b): the packs of a Java node that has left. Without it a run of 16 or more spent ordinals in a Java node's epoch hides the packs behind it from the version-1 ledger, so they leak, and `serverless.segments.node_pack.enabled` cannot be defaulted. It decides no default.

## Change

When a collector pass finds a node in an index register whose lease is gone, whose ledger is not Rust-written (it has no sidecar and its newest epoch is not a Rust reserve-ahead block), and which has no retired record, it retires it once:

1. List the children of `packs/<node>/` (`list_children`, an epoch directory a child, `// LIST:`), then each epoch directory's keys with `list_prefix`, up to 10,000 pack keys in all, at most 4 retirements a pass.
2. Record the result in a Rust-only blob `packs/index/retired/<node>` by create-if-absent: state `listed` with each epoch's ordinals as a bitmap, or state `over-cap` with none. A racing collector loses the create and reads the winner's record, so a node is listed once.
3. A `listed` node's sidecar is built from the record: every ordinal in an epoch's range that was not listed is marked collected, so the judge ([PackJudge](../../../crates/writer/src/pack/judge.rs)) is unchanged and sees only packs that exist. The ledger is advanced as before.
4. An `over-cap` node is logged, left in the index unjudged and never listed again.
5. The retired record is never deleted and a retired node's ledger and sidecar stay, because a returning node id would reuse its epochs.

Switch: retirement runs only with `serverless.segments.node_pack.collect` on and seeding on (`with_seeding`); with either off the collector makes no retirement request.

Not changed: Rust nodes (their ledger reserves ahead, so it cannot hide a pack), the judge, the reference sources.

## Acceptance criteria

- AC-1: a departed Java node's packs are found whatever gap its ledger hides them behind: a run of 20 spent ordinals between two packs, and the later pack is judged and deleted.
- AC-2: a node is listed once: a second collector, a restart, and N further passes make no retirement LIST, whether the node still has a live pack or none.
- AC-3: two collectors retiring one node at once record one result and both judge from it.
- AC-4: a node over the 10,000-key cap records `over-cap` on its first LIST, is judged by nothing and never listed again.
- AC-5: at most 4 retirement listings run in a pass.
- AC-6: a Rust node, a live node and a node with a sidecar are never retired.
- AC-7: with collection or seeding off no retirement request is made.
- AC-8: the record's bytes are pinned, and truncated and corrupt records are refused.

## Test plan

`crates/format/tests/pack_retired.rs` (AC-8), `crates/writer/tests/it/pack_retire.rs` over the emulator and its meter (AC-1 to AC-7), each written first and seen failing, with the planted violations named in each test.

## S3 request budget

Once a retired Java node: one LIST of `packs/<node>/` plus one a epoch directory it names, one create. Never on a read or write path; nothing per index, record or elapsed time.

## Switch

Behind `collect` and seeding, both off by default.

## Risks

A node with many epoch directories costs one LIST each; the 10,000-key cap bounds the keys, and an epoch count past 1,500 is `over-cap`.

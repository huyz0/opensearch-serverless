# A writer opens a shard on its first write

## Serves

R7.6, node assembly: nothing opened a shard for a write, so a created index answered "not open on this node". `ShardOpener` is what a `WriterRole` asks when a group's shard is not held; `HeadOpener` wins the shard's head and opens a new shard under the head's term.

## Change

`oss-node` (feature `writer`) gains `ShardOpener` (`open(role, index, shard)`), `WriterRole::with_opener`, `HeadOpener`, and `Writer::opener` for the assembly.

A write group for a shard the role does not hold calls the opener once (opens of one role are serialised, and the shard is looked up again after the lock, so racing groups open it once) and then goes on; a failed open answers every item 503 with the opener's reason.

`HeadOpener::open`:

1. acquires the shard's head for this process under the index's uuid; a live owner elsewhere refuses with its node id;
2. refuses, as not supported yet, a head that names earlier writers and a shard that already has a published commit: replaying and adopting what a predecessor left is the takeover piece;
3. reads the index's descriptor, which must still hold the uuid the write named, and opens the shard under the head's term with the descriptor's mapping.

## Acceptance criteria

- AC-1: a write to a new index's shard through a role with an opener opens the shard and is acknowledged; the shard's head names this node at a positive term.
- AC-2: a second write opens nothing: no head request is made.
- AC-3: groups racing for an unheld shard open it once: the head's term is the first's.
- AC-4: a shard whose head a live node owns is not opened here; its group goes to that node ([holder forward](../writer-holder-forward/SPEC.md)), and where the routes cannot forward every item answers 503.
- AC-5: a head that names earlier writers, and a shard with a published commit, are refused as takeover not supported yet; nothing is opened.
- AC-6: an index deleted and recreated under the name since the group was planned is not opened: the descriptor's uuid must match.
- AC-7: a role with no opener answers "not open on this node" as before.

## Test plan

`crates/node/tests/roles/writer_opener.rs` over the emulator, real head store and membership.

## S3 request budget

First write of a shard: the head's acquire, the register read that finds no commit, the descriptor read. Later writes: none of these. LIST 0.

## Risks

- An existing shard, or a head with predecessors, cannot be opened until takeover is ported to the node: those writes fail with 503 rather than risking a fork of the shard's history.

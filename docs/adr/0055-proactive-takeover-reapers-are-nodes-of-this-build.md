# 0055: Proactive takeover picks its reapers only among live nodes of this build

**Status:** Accepted after independent review (one round, changes applied), 2026-11-25

## Context

[ADR 0021](0021-proactive-takeover-from-the-node-log.md) lets a live node take the shards a dead node's write log still holds records for before a write asks: the log is replayed and published, so a reader sees the documents without waiting. Two reapers per dead node act, chosen by rendezvous hash of the dead node's id over the live ingest members.

In a mixed fleet the live members include Java nodes, which share the lease format but never run the pass. The mixed-fleet wide soak (several live nodes, one Rust owner killed each cycle, writes from both kinds) found that after a kill a Java reader missed about a quarter of the acknowledged documents of one shard until a write reached that shard, which takes it lazily. When the two reapers drawn for the dead node are Java nodes, nobody acts, and the shard waits for a write. A write to every shard after each kill makes the documents visible, which confirms the cause is a missing take, not a lost record.

Forces: requests scale with nodes and bytes; no lock, lease or leader for correctness; the take must not be raced; a Java node's code is frozen; every refresh already reads the live leases, which name each node's build in `version`.

## Decision

**Choose the reapers among the live ingest members whose lease `version` starts with this build's prefix (`oss-rust/`), behind `serverless.takeover.proactive.rust_reapers_only` (`OSS_PROACTIVE_RUST_REAPERS_ONLY`), default `true`.**

1. How a node decides it is the one: every node computes the same answer from the same refreshed leases. `reapers_for(dead_id, rust_members, reapers)` ranks by the rendezvous weight of `("reap", dead_id, node_id)`; rank 0 acts at once, rank 1 one refresh interval later and only for what is still not taken. Nothing is written to decide, so there is no election and nothing to race: a second node taking the same shard loses the same conditional create the lazy take already loses, and `done` keeps a node from repeating a finished pass. This is ADR 0021's rule with the candidate set narrowed.
2. A lease with no `version`, or any other prefix, is not a candidate. A fleet of one build is unchanged: the set is all members.
3. **Cost.** No request in steady state: the filter is over leases already read. After a crash, requests per orphaned shard are those of ADR 0021 (header read, take, replay, publish), paid by a node that runs the pass instead of by no node. A mixed fleet with no live Rust node has no reaper, and the shards are taken lazily as before.
4. **Default on, with a switch.** It changes nothing in a fleet of one build and removes a wait of unbounded length for a write in a mixed one. The one downside is that the work falls on fewer nodes in a mixed fleet (the Rust ones), which is why it is a setting: off keeps the old choice, and both are tested.

## Consequences

- A Java reader sees a killed Rust owner's acknowledged documents once a Rust reaper has published, without a write to the shard.
- A Java node that is the only ingest member left is not a reaper; unchanged from today, where it was one in name.
- The check reads a string prefix that two builds share by convention; the prefix is the one the forwarded get already uses to tell the builds apart.
- The prefix names the build family, not the capability. A node of this build with `OSS_PROACTIVE_TAKEOVER=false`, or an older build before the pass, is still a candidate and does not act; the stall then falls back to the lazy take, as before. Advertising the capability in the lease is a later change if it is measured to matter.

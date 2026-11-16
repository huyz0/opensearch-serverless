# Node leases: a node joins, renews and leaves by its own register

## Serves

Roadmap R4.1 ([plan](../../roadmap.md)): a Rust node is a member of a fleet, Rust or Java, by lease blobs alone. [ADR 0015](../../adr/0015-cluster-crate-and-node-leases.md), [ADR 0007](../../adr/0007-registers.md) (registers), invariants I-1 and I-2 (every request through `oss-blob`, no LIST).

## Change

- `oss-format` gains the three membership records the Java shell writes under `cluster/members/`: a node's lease (`lease-<node id>`, JSON, `NodeLease.toBytes` / `fromStream`), the members index (`members`, ids one a line), and the lease digest (`members-digest`, `LeaseDigest.encode` / `decode`); and their keys.
- A new crate `oss-cluster` (layer 2, beside `oss-meta`) gains `lease`: `Membership`, a port of `BlobLeaseMembership`: renew by compare-and-swap against the generation last written, with the duplicate-id refusal and the same-address restart rule; enrolment on the members index; revoke; clean release; self-validity with the skew margin (`ttl / 32`); refresh from the index, the digest and each lease whose copy has run out, pruning leases dead for 10 lifetimes; membership deltas; and `LivenessOracle` (`LeaseLivenessOracle`), with concurrent revocations of one node sharing one.
- Not changed: no node runs a lease yet (the writer's shard ownership, R5, will); no setting is added (the TTL is a constructor argument, 30 s by the Java's default, until a node wires it).
- One deliberate difference: the Java lists `lease-*` once when the members index is missing (a deployment from before the index). This never lists: a missing index is created naming the enrolling node. Every Java node at the ported revision writes the index when it joins.

## Acceptance criteria

- AC-1: for every lease in `fixtures/node-leases/cases.txt`, `NodeLease::parse` accepts exactly the leases the Java's `fromStream` accepts, and `write` of what it read is byte-identical to the Java's `toBytes` of what it read (`parsed.txt`); each line of `strict.txt` (a type the Java coerces, a repeated field) is refused.
- AC-2: the lease digest and the members index the Java writes for the recorded fleets decode to the same leases and ids, and Rust's encodings of them are byte-identical; a truncated or corrupt digest is refused at every cut.
- AC-3: a first renewal costs 3 requests (the lease created, the index read, the index swapped); every later one 1 swap; a node whose register was revoked by a successor gets `LeaseRevoked` and does not write over it; a live lease of the same id at another address is refused as a duplicate; one at the same address (a fast restart) is taken over.
- AC-4: `self_valid_at` is true before the first renewal, then exactly until `expiry - ttl / 32`, and `self_lapsed_at` is its complement once a lease was written.
- AC-5: `revoke` returns true and leaves a revoked lease only for an expired lease of the given process; false for a live one, an unreadable one, or a store error; true without writing for a released lease, an already revoked one, or one of a later process; 8 concurrent revokes of one dead node through `LivenessOracle` make one swap between them.
- AC-6: in a fleet of 3 on the emulator, a refresh sees every live member; a node that releases is gone from the others' next refresh, delivered as a departure; a node that joins is a join; a renewal alone is neither.
- AC-7: a refresh reads only the index (1 GET) while every held copy is unexpired; the digest writer (the live member with the lowest id) writes the digest at most once a third of a lifetime; a reader takes leases from the digest instead of reading each.
- AC-8: a lease expired for 10 lifetimes and naming no node-log epoch is unlisted from the index (and deleted where conditional deletes are verified); one naming an epoch is kept; one expired for less is kept.
- AC-9: an idle hour on a simulated clock, 3 nodes renewing and refreshing every 10 s, costs the digest writer at most 720 compare-and-swaps (the Java's number), every node no LIST, and a non-writer node at most 1,080 GETs; the numbers are rows in [request-budgets.md](../../request-budgets.md).
- AC-10: a Java node's lease, written by the shell's own `NodeLease` on the fs store, is read live by a Rust refresh, and a Rust node's lease is read by the Java's `fromStream`.

## Test plan

- AC-1, AC-2: `crates/format/tests/node_leases.rs` over `fixtures/node-leases/` (recorded by the Java's own classes, `NodeLeases.java`). Fails if a field is written in another order or form, or a Java-refused lease is accepted.
- AC-3 to AC-8: `crates/cluster/tests/` on the emulator with a simulated clock and the meter; a planted defect for each of: renewing over a revocation, a margin of 0, revoking a live lease, pruning an epoch-holding lease.
- AC-9: `an_idle_hour_costs_what_the_table_says` (cluster), and the rows checked by `check-budgets`.
- AC-10: `a_java_lease_on_fs_is_read_live` over a lease file the Java wrote into `fixtures/node-leases/fs/`, and the Java reading back what Rust wrote (`parsed.txt` round trip).

## S3 request budget

| Operation | Requests |
|---|---|
| first renewal | 1 create + 1 GET + 1 swap |
| renewal | 1 swap |
| refresh, copies unexpired | 1 GET |
| refresh, reading the digest | 2 GETs |
| digest write | +1 GET + 1 swap, once per ttl/3, one node |
| revoke | 1 GET + 1 swap |
| release | 2 GETs + 2 swaps (+1 conditional delete) |
| any of these | 0 LIST |

## Switch

None: membership is new behaviour with nothing to compare against, and costs only the requests above. No node runs it until the writer's shard ownership needs it.

## Risks

- A lease is the only place a successor finds a node's unpublished write-log epochs; pruning one that names an epoch would lose records. AC-8 covers it.
- Clock skew beyond the margin lets a holder and a taker overlap; the margin is for liveness only, revocation makes a skewed holder find out, and the write log's and shard heads' conditional writes fence its late writes (ADR 0015).
- The members index is a register every joining node swaps; a co-started fleet contends on it, which the jittered retry and the per-renewal re-attempt bound, as in the Java.

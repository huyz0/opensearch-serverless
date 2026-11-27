# Node pack collector, so packs nothing names are deleted

## Serves

Parity plan R11.1 (node packs) and [ADR 0025](../../adr/0025-node-pack-collection.md) (accepted): with `serverless.segments.node_pack.enabled` on, a pack nothing deletes leaks storage, so ADR 0024 keeps the switch off outside bounded measurement runs. This builds the collector so the switch can be measured and then defaulted. The ADR holds the design and its reasons; this spec holds what is built, in slices, and what proves each.

## Change

Everything is behind a new node setting `serverless.segments.node_pack.collect` (node scope, default false, env `OSS_NODE_PACK_COLLECT`); it does nothing while `node_pack.enabled` is off, and with it off a packing node behaves as after the concurrent-publish slice. Four slices, each its own PR, each test-first:

1. **Ledger rotation and the sidecar.** `PackLedger` rotates an epoch every 1,024 ordinals and reserves `high` ahead (a swap every 8 packs); the Rust-only sidecar register `packs/<node>/collected` holds a 128-byte bitmap and `high` per live epoch (144 bytes an epoch, rotation stops at 1,500 live epochs). The Java version-1 ledger stays readable and is advanced as far as its contiguous prefix goes.
2. **Index registers.** 16 registers `packs/index/<hash mod 16>`: a Rust writer adds its node before its first pack (always-write, fails the publish if it cannot), the collector removes by compare-and-swap on a generation read before the sidecar, both retry at most 16 times with jitter.
3. **The collector pass.** One collector per 10-minute window chosen by `hash(window) mod n` over the sorted live membership (no lease, CAS decides); a pack is judged only past 15 minutes of age (`HEAD` `LastModified` against `Date`) and only after `PackReferences` (an allow-list over files that persist a manifest, a bucket manifest or a file location) names nothing; the swap deadline is on the boot-time clock.
4. **Java leftovers (ADR decision 1, option b).** The resumable seeding LIST under `packs/index/seeded`, Java members added to the index once per join, and one recorded LIST per retired Java node into `packs/index/retired/<node>`.

Not built: a recheck at a fixed interval (a referenced pack is rechecked on a doubling backoff instead, ADR 0025 decision 2), a repack, any collector on the request path, collection while a reference source cannot be read.

## Acceptance criteria

- AC-1: with `collect` off (or `enabled` off) no ledger, sidecar, index or collector request is made beyond what a packing node makes today (existing node-pack tests pass unchanged).
- AC-2 (slice 1): a ledger reserves `high` ahead and rotates at 1,024 ordinals; a Java-format reader reads the version-1 ledger after a Rust advance and after a dropped epoch.
- AC-3 (slice 1): after a restart the collector reads the sidecar once and re-reads headers only of unmarked ordinals (live packs), not 1,024 an epoch; at 1,500 live epochs rotation stops and the node logs it.
- AC-4 (slice 2): an add before the first pack always writes (its generation moves); a removal against an older generation fails and retries within 16 attempts; interleaved add and removal never loses a node that has a live pack.
- AC-5 (slice 3): exactly one node runs a window's pass when membership agrees; two disagreeing nodes both running is safe (CAS decides, nothing is deleted twice or while referenced).
- AC-6 (slice 3): a pack younger than 15 minutes, or named by any reference source, is never deleted; each reference source in the ADR has a test with a planted reference; a source that cannot be read stops judging and says so.
- AC-7 (slice 3): the persister allow-list check fails on a planted new persister (selftest) and has a row in `docs/mechanical-gates.md`.
- AC-8 (slice 3): a pass with nothing due makes no request beyond one sidecar GET per node with a live epoch; N idle passes make no PUT and no LIST beyond the window's inventory LIST.
- AC-9 (slice 4): a second collector and a restart after seeding issue zero seeding LISTs; two simultaneous first collectors seed at most one extra page; a crash mid-seed resumes from the cursor; a Java node that joins after seeding, leaves and is retired; a dead node with a live pack causes no second LIST across N passes; a retirement over the 10,000-key cap records `over-cap` and no second LIST follows; N passes with unchanged Java membership make zero register PUTs.
- AC-11 (slice 3): a pack found referenced is rechecked at 15 minutes and then at doubling intervals, with no request between rechecks; a pack whose last referencing manifest has since dropped it is deleted after two unreferenced sightings at its next recheck; a restart starts every live pack's backoff again.
- AC-10: the setting is in the node registry with default and bound, `OSS_NODE_PACK_COLLECT` is read like the other pack variables, and a collector's tracked bytes are reserved from the node's memory pool and stop at the cap with a log line.

## Test plan

`crates/writer` and `crates/node` over the emulator and its meter (request counts are asserted exactly, as in node-pack-wiring); `crates/node/tests/roles/writer_packs.rs` for the end-to-end collect; a differential check of the ledger against the Java-written version-1 bytes in the fixtures; the planted violation in `scripts/selftest-checks.py`. Each test written first and seen failing. Slice PRs name the ACs they cover.

## S3 request budget

Steady state, all provisional and as in the ADR: 144 inventory LISTs a day per cluster plus paging; at most 144 sidecar PUTs a day per node that collected; 16 index GETs per pass; one sidecar GET a pass per node with a live pack; a ledger swap every 8 packs (about 12% more PUTs); seeding `ceil(node ids ever seen / 1,000)` LISTs once; at most one LIST per retired Java node. Nothing scales with documents, indexes or time per index; each LIST carries `// LIST:`; no LIST on a read, write or startup path.

## Switch

`serverless.segments.node_pack.collect`, default false. The default flips only after the R11 measurement shows packs win and the collector's request counts match this budget; per-index and cluster-register overrides are separate follow-ups. Memory: tracked packs are a key plus shard keys, capped per entry and in total, reserved from the pool.

## Risks

A pack deleted while referenced (a lost reference source): AC-6 and the allow-list gate hold it, and judging stops when a source cannot be read. A repeated LIST on elapsed time: AC-8 and AC-9. A paused collector deleting after a newer one judged: the delete is on a pack already past two unreferenced sightings and the swap deadline is on the boot-time clock, not a lease.

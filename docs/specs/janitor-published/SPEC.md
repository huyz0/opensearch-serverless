# Janitor: sweep what a node has just published

## Serves

Parity plan R8.4, third piece, invariants I-1 and the S3 rule (requests scale with nodes and bytes; an idle index makes none). Ported from `reconcile/PublishedSweeps.sweepPublished` and `sweepCanFindSomething` at `3920dc36`. Follows [janitor-gc](../janitor-gc/SPEC.md); decision: [ADR 0020](../../adr/0020-janitor-crate.md), whose "a commit that dropped a file triggers a sweep" this delivers.

## Change

- `oss_janitor::published::Sweeps`: the driver that decides which shards a pass sweeps and keeps what each sweep learned, over `gc::Collector`.
  - **Driven by publishing, not by holding.** A shard nobody writes to makes no garbage, so sweeping held shards on a timer would pay a listing per shard per pass to be told nothing changed. A pass names the shards it just published, with the file names of the commit it published; a shard is swept only when its manifest lacks a file the previous publish's had (a merge or a delete; `segments_N` is ignored, every commit replaces it), when its term is above the previous publish's (a file dropped in the live term is not judged until the term moves on), when blobs of it are on watch for the grace, when it is pending (forced, since a reaped view may have freed something, or its last sweep failed), or when this node has never published it since opening it. A publish that only added segments, most of them, costs nothing.
  - **Only shards the caller holds.** The caller passes only shards this node owns and calls `forget` when it lets one go, which drops the shard's state and its memory.
  - **Per shard, not per pass.** A shard whose sweep fails (a malformed manifest, a store failure) is reported with its error and does not stop the others; it is pending, so the next pass tries again even if it publishes nothing.
  - What a sweep reports is `gc::Sweep`: the keys deleted, the keys a dry run would delete, and how many are watched.
- `gc::Memory::forget` (the shard's entry, public).
- Not in this change: reader refresh, view reaping, tombstone and reclaim scheduling, the orphan sweep, relisting avoidance, the node wiring.

## Acceptance criteria

- AC-1: a shard never swept is swept by its first publish; a later publish that only adds files sends no request at all.
- AC-2: a publish whose manifest dropped a file the previous publish's had is swept, even if that publish was skipped; one that only lost `segments_N` is not.
- AC-2b: a publish in a later term than the previous one is swept.
- AC-3: a shard with blobs on watch is swept by a pass that published nothing of it, until none is watched; then a pass that publishes nothing costs no request.
- AC-4: `force` makes the next pass sweep the shard once.
- AC-5: `forget` drops the shard's state and memory: its next publish sweeps as a first one, and it deletes nothing on it.
- AC-6: a pass with nothing published and nothing watched sends no request.
- AC-7: a shard whose sweep fails is reported with its error, the others are swept, and the failed shard is swept again by the next pass, published or not.
- AC-8: the requests of a pass are those of the sweeps it runs and nothing else.

## Test plan

`crates/janitor/tests/published.rs` over the emulator with its meter, as the gc tests: manifests written with `encode_manifest`, blobs with `put`, one test for each criterion.

## S3 request budget

Per pass: none for a shard the gate skips; otherwise a sweep's (one register read, a LIST page per 1,000 keys of the shard, a DELETE batch per 1,000 blobs). The cost follows write activity that dropped a file; an idle deployment pays nothing.

## Switch

The janitor's one switch, `serverless.janitor.dry_run`, carried by the collector.

## Risks

- A gate that skips a sweep it should run only delays a deletion (storage); the first sweep after opening, `force`, and the watch list bound the delay.
- State growth: one entry per shard the node holds, the term and file names of its last published commit (at most the manifest's file limit); `forget` bounds it by the shards held.

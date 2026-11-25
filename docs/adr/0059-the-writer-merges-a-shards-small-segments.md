# 0059: The writer merges a shard's small segments

**Status:** Accepted after independent review (one round, 2026-11-25: its blocking findings on reclaim, pins, soft deletes and memory are answered in decisions 3, 4 and 6)

## Context

The writer opens each shard's `IndexWriter` with no merge policy, so nothing ever merges: every refresh writes a segment and the shard's commit lists all of them. Each acknowledged bulk forces a refresh, so a shard that takes a bulk a second holds thousands of segments within an hour. A segment is published as one immutable blob, and a node that opens the shard (a reader, a writer taking over, a Java coordinator) reads each segment's files in turn. Cold-open time therefore grows with write history, and the object store is touched once per file of every segment.

The wide mixed-fleet soak shows the cost. After the first cycle (2,700 documents over 4 shards) the store held 797 segment blobs. Stack dumps of a Java node's hung `_search` show it inside its own cold open, in `ObjectStoreIndexInput.fetchSpan` reading from the store one range at a time, still running six minutes after it began; a Rust owner serving the Java shard action sat in `SearchService::open_shard` waiting on a cold open of the same kind. The Java change that stops a search waiting on its own cold open ([OpenSearch #183](https://github.com/huyz0/OpenSearch/pull/183)) lets a search ask another copy, but every copy opens cold, so the soak still fails.

The engine already has a tiered merge policy (`lucene_index::merge_policy`, with Lucene's defaults) and runs merges inside `IndexWriter::commit`. The shard's directory is in memory (`ByteBuffersDirectory`), so a merge allocates a merged copy of its inputs while it runs.

Forces: no extra PUT per write; requests scale with nodes and bytes; bounded memory with reservation before allocation ([ADR 0004](0004-bounded-memory-and-streaming.md)); write p99 must not regress; segments are immutable and named by content, so a merged segment is a new blob and the old ones are reclaimed by the janitor.

## Decision

**The writer opens each shard's `IndexWriter` with a tiered merge policy behind `serverless.writer.merge.enabled`, deletes nothing a merge replaces, and merges one shard at a time per node within a reserved memory bound.**

1. **Policy.** `MergePolicyConfig` with `segments_per_tier` 8, `max_merge_at_once` 10, `max_merged_segment_size` 512 MiB and the engine's other defaults. These three are settings, as is `max_full_flush_merge_wait_millis` (the engine's 500 ms): a merge is cut short at that deadline and resumes at the next refresh, so the segment count is bounded after the shard has had refreshes to catch up, not at every instant. Settings are cluster defaults and node-scope, with the switch overridable per index (`docs/principles.md`, settings).
2. **When.** In the refresh's commit, as `IndexWriter::commit` does. No background thread; a scheduler off the refresh path is a follow-up if the spec's AC-8 fails.
3. **Memory.** The merge runs in the shard's in-memory directory and holds its inputs and its output at once. The policy is wrapped so that it proposes a merge only after the node has reserved twice the merge's input bytes from the memory pool, and proposes none when the pool refuses (the refresh succeeds, the merge is tried at the next one). A node-wide permit lets one shard merge at a time, so the peak a node adds is one merge's 2x, not one per shard. The spec's AC-4 measures the peak.
4. **Soft deletes.** The writer keeps its soft-deletes retention at `None`: a merge keeps every soft-deleted document and recovery source, so history a Java writer taking over replays from is never dropped. The cost is that a merge does not reclaim the bytes of deletes and updates; the segment count is bounded, the bytes are not. A retention policy that drops history is a later decision with its own takeover tests.
5. **Publish.** The next publish uploads the merged segment and no replaced one; the manifest lists live segments only. A segment not yet uploaded when a merge replaces it is never uploaded, so a merge can lower a publish's PUT count; a merged segment's bytes are uploaded once more per tier. Packed and unpacked publishing are both tested; a segment that gained a file (doc-values update, soft delete) is rewritten into a new blob as today.
6. **Reclaim: none in this change.** The janitor deletes a blob only from a term container older than the published manifest's term ([janitor-gc](../specs/janitor-gc/SPEC.md)); blobs of the live term are never judged. A merged-away segment sits in the live term's container, so it stays until a takeover bumps the term. This ADR adds no deletion path, which has two consequences, both accepted: storage for a shard grows with the bytes it has merged (at most about the number of tiers times its data, until a takeover), and every reader, point-in-time view and snapshot that names a replaced segment keeps working with no pin, because nothing it names is deleted. Reclaiming same-term blobs needs pins wired through the janitor and is a separate ADR, because the term rule is the janitor's safety argument.
7. **Crash.** A merge changes only the writer's in-memory directory. A crash before the next publish loses it and takeover replays the log onto the last published commit, as it does for any unpublished refresh.
8. **Cost.** No blob request is made by a merge. The cost is writer CPU and memory and bytes uploaded at publish, measured by the spec's AC-8 and AC-9 on an emulator, so provisional.
9. **Switch.** `serverless.writer.merge.enabled`. Default on only if, with it on: refresh and acknowledgement latency is within 5% (AC-8), a search and a cold open of the same shard are not slower and the soak's cold opens fall as AC-7 says, no extra blob request is made, and the crash and correctness tests pass. Otherwise off, and the soak sets it. The numbers are provisional, so the default is set in the PR that has them, not in this ADR. Off keeps today's behaviour, pinned by the existing tests; both modes are tested.

## Consequences

- A shard's commit lists a bounded number of segments (about `segments_per_tier` per tier), so a cold open reads a bounded number of files instead of a number that grows with write history. This is the cure for the soak's hang on the Java and the Rust side and holds for every node that opens the shard.
- A refresh that merges takes longer than one that does not, on the write path; write p99 is the thing to measure.
- A reader following pushed commits fetches a merged segment it has not seen: bytes, once per merge, and any warm per-segment cache of the replaced segments is wasted. It reads a correct commit throughout.
- Replaced blobs accumulate in the live term until a takeover (decision 6). Storage is the price of keeping readers, point-in-time views and snapshots safe without pins.
- Takeover reads fewer files; a Java node taking over a Rust-merged shard reads Lucene segments as it does today (AC-6).
- `docs/invariants.md`, `docs/request-budgets.md`, `crates/writer/README.md` and `docs/architecture.md` change when it lands.

## Alternatives rejected

- **Lazy per-segment open and a bounded Rust handler** (option 2). Opens still read every segment eventually, and the number of segments keeps growing; it is a bound on a cost that keeps rising. Kept as a follow-up if cold opens are still too slow after merging.
- **Merging on a reader or the janitor.** A reader's merged copy cannot be published under the writer's fencing; a janitor that merges needs write access to the shard's commit chain, which is the writer's by compare-and-swap.
- **Publishing fewer, larger segments by refreshing less.** Changes acknowledgement semantics (`forced_refresh`), which the Java coordinator relies on.

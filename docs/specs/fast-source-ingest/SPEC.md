# Fast source ingest: a plan-based path in front of oss-doc

## Serves

The write-path cost goal (roadmap stage 3, [roadmap](../../roadmap.md)) and [indexing](../indexing/SPEC.md): `Shard::index` spends about 57% of a log-shaped write turning a source into a Lucene document, and lucene-rust's native source ingest (`lucene_engine::ingest`) does the same reading 9-12x faster on parse alone. Keeps [ADR 0019](../../adr/0019-document-indexing-crate-below-reader-and-writer.md): one crate decides what a source becomes.

## Change

- `oss-doc` gains `fast` (`crates/doc/src/fast/`): a `FastIngest` per shard that compiles the index's mapping into an ingest `Plan` (`lucene_engine::ingest::register`) and reads a source through `ingest::prepare`, giving the `ExplicitDocument`s the writer applies (`Prepared::finish` adds the sequence number, primary term and version).
- **The plan only names fields the shard's `Indexer` has already numbered.** The field numbers in the plan are the ones `Indexer` registered with the engine's `IndexWriter`, so numbering stays in the order the Java's `IndexingChain` first sees fields ([indexing](../indexing/SPEC.md) AC-1). A source with a field outside the plan is declined, goes through `document::parse` and `Indexer::document` as today, and that registers the field; the plan is then recompiled.
- **A decline is never an error.** Anything the plan does not reproduce exactly, and any mapping it cannot express (dynamic or unmapped fields, `ignore_malformed`, a field that needs `_field_names`, `copy_to` to a non-text target, a type outside the plan's kinds, doc-values skip indexes, a mapping update, a source that is not strict JSON), sends the source to oss-doc, which gives the result and the exact error.
- `Shard::index` and the reader's overlay build call the fast path first. `oss-doc` stays the complete path and the oracle.
- Sources reach `prepare` whole, as the writer already holds them (bounded by the HTTP body bound); `chunkjson` stays the reader of `_bulk` bodies and of mappings.
- No switch: Huy's decision, and the measurements below must show no downside. A measured downside stops the work and is reported instead of getting a setting.

Not changed: sequence numbers, versions, deletes, soft deletes, tombstones, the memory reservation, the publish path.

## Acceptance criteria

- AC-1: for the log corpus, a wide mapping's edge documents and a 3,000-document generated corpus, the documents the plan gives equal those oss-doc gives (stored, inverted, doc values, points and registered fields, with the same field numbers; terms without positions compared without frequency, which a document-only field does not keep).
- AC-2: a document oss-doc refuses is never read by the plan, and every decline (an unmapped or not yet numbered field, a repeated key, an array of arrays, a coercion, odd whitespace) gives oss-doc's document.
- AC-3: the segment fixtures of [indexing](../indexing/SPEC.md) (AC-1, AC-2, AC-4, AC-5) pass with the plan in front of the writer, field infos included.
- AC-4: after oss-doc grows the mapping dynamically or numbers a new field, the plan is recompiled and reads the next document of that shape, equal to oss-doc's.
- AC-5: the plan is compiled again only when the mapping grew or a field was newly numbered, never per document.
- AC-6: on log-shaped documents (release, in-memory directory, provisional) building the Lucene document through the plan is at least 2x the complete path's rate.
- AC-7: `Shard::index` on the 100,000-document log corpus is at least 1.4x the complete path's MB/s, and its p99 per call is no worse than 105% of the complete path's (the same loop with the plan declining every source).
- AC-8: the writer's replay and takeover tests pass unchanged with the plan in front.
- AC-9: a 10,000-document `_bulk` issues the same number of PUT-class and GET-class requests, and the same sequential depth, as before (the request meter).

## Test plan

AC-1, AC-2, AC-4: `crates/doc/tests/fast_differential.rs`, which runs both paths over the corpora above (seen failing against a plan that drops norms). AC-5: the same file counts compiles. AC-3, AC-8: the writer's segment fixture and replay tests. AC-6: `crates/doc/tests/fast_bench.rs`, an ignored release test. AC-7: the same loop through `Shard::index`, run by hand with the plan declining every source and again with it on (the comparison needs the plan switched off, so it is a recorded measurement). AC-9: the existing bulk budget tests.

## S3 request budget

Unchanged: building a document touches no blob.

## Switch

None, as above.

## Risks

- A document the plan reads that oss-doc would refuse or read differently (coercions, date formats, numeric edges): AC-1 and AC-2 compare both on a generated corpus including edge values.
- A heavier dependency: `lucene-engine` is already used by the reader with default features off; the ingest module adds no crate.
- The engine's plan registry is process-wide and capped (16,384 plans): a mapping that changes constantly would exhaust it; the fast path stops registering and declines at the cap.

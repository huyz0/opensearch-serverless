# Index lifecycle

## Serves

The plan's index lifecycle milestone: create and delete an index (no REST yet), with the delete's reclaim queue, idempotent on a retried create. Follows the metadata writes milestone ([routing-cache](../routing-cache/SPEC.md)). Decision record: [ADR 0012](../../adr/0012-index-deletes-and-reclaim.md), under [ADR 0008](../../adr/0008-metadata-crate.md).

Invariants served:
- **Fencing by conditional create and delete:** a delete removes the descriptor at the generation it read, and every later step is scoped to the deleted uuid, so a new index of the name is never touched.
- **S3 requests are the bill:** a create is one PUT; a delete's requests scale with its shards and bytes; a drain with nothing due is one LIST.

## Change

Ported from huyz0/OpenSearch at `3920dc36`, `serverless/shell/src/main/java/org/opensearch/serverless/metadata/`:
- `IndexLifecycle.java`: `createIndex`, `deleteIndex`, `reclaim`, `purgeShardData`.
- `ReclaimQueue.java`: `Intent`, `enqueue`, `drain`, `setDelayMillis`, `bucketStart`.

### In `oss-format`

- **`ReclaimIntent`** writes an intent byte for byte as the Java concatenates it, and parses it as the Java's `parse`: `had_writer` is false only for the literal `false`. A repeated field is refused, as Jackson's strict duplicate detection does. A fractional shard count or one past 4,096 is refused, where the Java would take it.
- **`reclaim_bucket` and `reclaim_bucket_start`**: the UTC minute `uuuuMMddHHmm`, years 1970 to 9999. **`reclaim_intent_key`**: `reclaim/<bucket>/<index>#<uuid>`.

### In `oss-blob`

- **`BlobStore::validate_key`** checks a key with no request and no meter entry: S3's rule by default, the filesystem store's own (255-byte components, 64 components).

### In `oss-meta`

- **`IndexLifecycle::create_index`** checks the name, then the longest shard key the layout needs (`segments/<index>#<uuid>#<shards-1>/manifest`) against the store, then creates the descriptor. A refusal for a name holding the same uuid answers that record's generation: a retry after a lost response.
- **`IndexLifecycle::delete_index(name, expected_uuid, now)`**, as the Java's `deleteIndex`:
  1. resolve the name, one read; no index, or another incarnation than `expected_uuid`, answers `None`;
  2. where deletes are conditional, read the heads, then file the intent;
  3. delete the descriptor at the generation read; a change since answers `None`;
  4. delete the heads at the generations read (conditional), or release them (tombstone mode);
  5. detach the index from the aliases its hint names;
  6. purge the shards' bytes, except a shard a pin names, unless deletes are conditional and a shard had a writer: then the reclaim purges.

  It answers what was left: aliases still naming the index, and shards pinned or not cleared.
- **`IndexLifecycle::reclaim(intent)`**: nothing when the index is live under the intent's uuid; otherwise delete the uuid's heads, and purge when a shard had a writer or a head reappeared.
- **`ReclaimQueue`**: `enqueue` is one PUT under the minute of now plus the delay (two minutes by default). `drain(now, owns_bucket, reclaim)` walks the queue in key order and stops at the first bucket not yet due. A due intent is taken (deleted) when `reclaim` succeeds or its body is no intent; one whose GET or `reclaim` fails is kept.
- **`ShardHeads` and `ShardPins`**: traits for the heads and pins a delete needs. No implementation ships: nothing can wire a delete or a drain until the writer's heads and the snapshots are ported (ADR 0012). Tests use recording fakes.
- A drain's page that ends inside another node's bucket resumes past it (`<bucket>0`), so such a bucket costs at most one more LIST.

### Not in this change

REST (`PUT /<index>`, `DELETE /<index>`) and template resolution, which come with the coordinator. Scheduling the drain, which is the node runtime's. The shard heads, snapshot and point-in-time pins themselves. A unit layout is carried as the record gives it; computing one is the shard modes' work.

## Acceptance criteria

- AC-1: A create is one PUT on each backend, and a retried create answers its own generation for two GETs and the refused PUT; another uuid or an alias of the name is refused. A bad name, a uuid holding `#`, or a shard key the store cannot hold is refused before any request: a 250-byte name with ten shards on the filesystem store, which an object store takes. Tests: `a_create_writes_the_descriptor_in_one_request`, `a_retried_create_answers_its_own_record_and_no_other`, `a_create_refuses_a_bad_name_or_unusable_layout_before_any_request`.
- AC-2: A conditional delete with no writer reads once, files one intent under the due minute, deletes the descriptor, deletes the heads at the generations read, detaches aliases (a read and a swap each, counted) and purges every shard with one LIST each, leaving another incarnation's bytes. A failed intent PUT leaves the index whole. Tests: `a_conditional_delete_files_its_intent_first_then_removes_everything`, `the_intent_is_written_before_the_descriptor_goes`, `a_delete_detaches_the_index_from_its_aliases`.
- AC-3: A shard with a writer keeps its bytes until the intent falls due, and the reclaim then purges what the writer published since. A tombstone-mode delete releases the heads, purges at once and files no intent. Tests: `a_shard_with_a_writer_leaves_its_bytes_to_the_reclaim`, `a_deleted_writers_bytes_go_when_the_intent_falls_due`, `a_tombstone_delete_releases_heads_and_purges_without_an_intent`.
- AC-4: A delete of another incarnation, a missing name or an alias is one read and writes nothing. A delete that loses a race to a descriptor update answers `None` and leaves only its intent, which the reclaim drops without touching the live index. Tests: `a_delete_of_no_such_incarnation_reads_once_and_writes_nothing`, `a_delete_that_loses_a_race_leaves_only_an_intent_the_reclaim_drops`.
- AC-5: A pinned shard keeps its bytes; unreadable pins purge nothing; a shard over 1,000 blobs is purged in two LISTs; a failed shard is counted. A reclaim of a recreated name removes only the dead uuid's heads and bytes, purging only when a shard had a writer or a head reappeared. Tests: `pinned_shards_keep_their_bytes_and_unreadable_pins_purge_nothing`, `a_purge_pages_through_a_large_shard_and_counts_a_failure`, `a_reclaim_removes_only_the_dead_incarnation`.
- AC-6: The queue: an intent is one PUT with the Java's bytes; it is due once its whole minute has passed; a drain with nothing due is one LIST; a due intent costs a GET and a DELETE; the walk stops at the first bucket not due with no LIST past it, skips buckets this node does not own (a page ending inside one resumes past it, one more LIST) and stray keys, keeps an intent whose GET or reclaim fails and moves past a thousand kept ones, drops a corrupt or oversized one, and pages past 1,000. Tests: `a_drain_lists_nothing_past_the_first_bucket_not_yet_due`, `a_drain_moves_past_a_thousand_kept_intents`, `a_page_ending_in_another_nodes_bucket_skips_the_rest_of_it`, `an_intent_is_one_put_filed_under_the_minute_it_falls_due`, `an_intent_is_due_only_once_its_whole_minute_has_passed`, `a_drain_stops_at_the_first_bucket_not_yet_due`, `a_bucket_this_node_does_not_own_is_left_to_its_owner`, `a_failed_reclaim_keeps_its_intent_and_a_corrupt_one_is_dropped`, `a_failed_get_keeps_the_intent`, `a_drain_pages_through_a_thousand_due_intents_and_more`.
- AC-7: The intent format and buckets: Java bytes, the lenient `had_writer`, skipped and repeated fields, missing or mistyped fields, truncation, minute buckets and their inverse, the key. Tests: `an_intent_is_written_byte_for_byte_as_the_java_concatenates_it`, `a_written_intent_reads_back_and_a_short_buffer_says_what_it_needs`, `had_writer_is_false_only_for_the_literal_false`, `unknown_fields_are_skipped_and_order_does_not_matter`, `an_intent_missing_or_mistyping_a_field_is_corrupt`, `a_repeated_field_is_corrupt_as_jackson_strict_detection_refuses_it`, `a_truncated_or_trailing_intent_is_corrupt`, `a_bucket_is_the_utc_minute_a_due_time_falls_in`, `a_bucket_starts_at_its_minute_and_round_trips`, `a_name_that_is_not_a_real_minute_has_no_start`, `an_intent_key_is_filed_under_its_bucket_by_name_and_uuid`.
- AC-8: `validate_key` sends nothing: the filesystem store refuses a 256-byte component and takes 255, the emulator takes 256 and refuses an empty or 1,025-byte key. Tests: `invalid_arguments_fail_before_any_attempt`, `invalid_arguments_fail_before_any_request`.

## Test plan

`crates/meta/tests/descriptors/index_lifecycle.rs` and `reclaim.rs` over the emulator, S3 on loopback and the filesystem store, with exact meter counts; the race through the test store's hook; heads and pins through recording fakes. `crates/format/tests/reclaim_intents.rs` for the bytes.

## S3 request budget

S shards, B blobs in all, A aliases hinted, each read and swapped once when uncontended; the filesystem's conditional delete adds one GetRange. Deletes are sequential, one request each: S3 does not bill DELETE.

| Operation | PUT-class | GET-class | LIST | DELETE | Depth |
|---|---|---|---|---|---|
| `create_index` | 1 | 0 | 0 | 0 | 1 |
| `create_index`, retried | 1 | 2 | 0 | 0 | 3 |
| `delete_index`, conditional, no writer | 1 + A | 1 + A | S + B/1000 | 1 + B | 3 + 2A + S + B |
| `delete_index`, a writer | 1 + A | 1 + A | 0 | 1 | 3 + 2A |
| `delete_index`, no such incarnation | 0 | 1 | 0 | 0 | 1 |
| `enqueue` | 1 | 0 | 0 | 0 | 1 |
| `drain`, nothing due | 0 | 0 | 1 | 0 | 1 |
| `drain`, n due, U buckets of others ending a page | 0 | n | 1 + n/1000 + U | n | 1 + 2n + U |
| `reclaim`, live index | 0 | 1 | 0 | 0 | 1 |

Heads add one read a shard and one delete a head when they are ported. None of this is on a search or write path.

## Switch

None. The delete mode is the store's, probed as before; the reclaim delay is `ReclaimQueue::with_delay_millis`, two minutes by default as in the Java, which Java nodes on the bucket share.

## Risks

- A delete without real pins would purge bytes a Java shallow snapshot names, and a drain without real heads would leave a Java writer's heads: no implementation ships, so neither can be wired yet (ADR 0012).
- A drain on every node each tick costs a LIST per node per tick, as the Java's does; the runtime that schedules it records its choice.
- A purge that fails on a conditional delete with no writer is counted, not retried: its intent's reclaim does not purge again. That leaves storage nothing serves, as in the Java.
- The purge's sequential deletes are a round trip a blob; the REST change that wires a delete decides where that depth goes (ADR 0012).

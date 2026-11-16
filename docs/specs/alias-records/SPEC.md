# Alias records

## Serves

The plan's metadata-writes milestone, sixth part. An alias or data stream lives in the same register namespace as index descriptors, so a name is one or the other, and resolving a name is one read. Rust nodes read, create and move aliases a Java node wrote, and the reverse. Follows [cluster-config](../cluster-config/SPEC.md).

Invariants served:
- **I-1:** every store call is metered.
- **Fencing:** an alias changes only by conditional create or compare-and-swap. Name uniqueness between aliases and indexes is the store's put-if-absent on one key.
- **Requests scale with nodes, not indexes:** resolving any name is one read.

## Change

Decision record: [ADR 0008](../../adr/0008-metadata-crate.md) (Accepted). Records' bytes are the Java's.

Ported from huyz0/OpenSearch at `3920dc36`:
- `serverless/shell/src/main/java/org/opensearch/serverless/cluster/AliasRecord.java`: `toBytes`, `fromStream`, the constructor's write-member rule, and `writeIndex`, `writesTo`, `mutationPending`, `creationPending`, `protectsHint`.
- `metadata/DescriptorStore.java`: `createAlias`, `updateAlias`, `resolve` and `resolveRaw`, sharing `createRegister` with an index's create.

### In `oss-format`

`AliasRecord` is an alias or data stream record: its name, its indices, the data stream's generation and timestamp field, the write member, and a pending mutation (`until`, `creating`, `additions`).

- **`write`** writes the Java's `toBytes` byte for byte, in its field order:
  - a data stream's fields only for a data stream;
  - the write member only when it is a member of a plain alias;
  - the mutation only while one is pending.

  It refuses a record the Java cannot read back: no indices, a mutation deadline not above zero, or an empty addition.
- **`parse`** reads as `fromStream`:
  - the name and at least one index are required;
  - unknown fields are skipped;
  - a write member that is not a member of a plain alias is dropped;
  - a mutation's fields are checked, and a repeated known field is refused. Jackson also refuses a repeated unknown field, which this does not check, as for an index record. No node writes one.

  It refuses values of a JSON type the Java never writes there, which the Java would coerce: a name that is not a string, a flag that is not a boolean, a generation that is not an integer. It also refuses bytes after the record, which Jackson ignores. Neither occurs in anything a node writes.
- **`NameRecord::parse`** now decodes an alias register into an `AliasRecord`, where it used to hand back the bytes. `NameRecord::index_only` recognises an alias or tombstone without decoding it, for the name listing, as the Java's listing does.

### In `oss-meta`

- **`DescriptorStore::create_alias(record, now, buf)`** is an index's create on the same key:
  - a put-if-absent;
  - a tombstone-sized read only when the name is taken;
  - a swap over a tombstone whose claim has passed.

  So an alias and an index never share a name.
- These two are the register step only, as the Java's `DescriptorStore` methods are. Publishing an alias goes through the alias mutation protocol, which prepares its members' `aliased_by` hints first, so that a deleted index leaves its aliases. That protocol comes next.
- **`update_alias(record, expected, buf)`** is one swap from the generation read, which makes moving an alias between indexes atomic. It bumps no change stripe, as in the Java: stripes confirm indexes.
- **`resolve(name, buf)`** answers what a name stands for, with its generation. An alias still being created (`creating`) resolves as absent, with its register's generation, as the Java's `resolve`. `resolve_raw` keeps it, and `Target::Alias` now carries the record.

### Not in this change

The alias mutation protocol that prepares members' `aliased_by` hints (`AliasMutations`), deleting an alias, the `_aliases` REST actions, and data stream rollover.

## Acceptance criteria

- AC-1: Every case in `fixtures/aliases/cases.txt` reads as the Java's own `AliasRecord.fromStream` reads it (`parsed.txt`): the same `toBytes`, write index, write target, pending creation and mutation, and protected hint, or refused where the Java refuses. A repeated known field is refused. What the Java writes reads back and writes the same bytes. Each case in `strict.txt`, which the Java reads by coercion, Rust refuses. Tests: `every_case_reads_and_writes_as_the_javas`, `values_the_java_coerces_are_refused`, `a_write_member_the_java_drops_is_not_kept`, `a_repeated_field_is_refused_as_jackson_refuses_it`, `a_repeated_unknown_field_is_kept_where_jackson_refuses_it`.
- AC-2: A record built in Rust writes the Java's bytes, a data stream's and a mutation's included. A record the Java could not read back is not written, and a short buffer is refused. A name register holding an alias decodes as one. Tests: `a_built_record_writes_the_javas_bytes`, `a_record_the_java_cannot_read_back_is_not_written`, `a_name_register_holding_an_alias_reads_as_one`, `an_alias_or_a_tombstone_is_told_apart`.
- AC-3: Requests, counted on each backend:
  - creating an alias is 1 `Put`;
  - creating one over a live name is 1 `Put` and 1 `Get`, and is refused;
  - creating one over a tombstone is 1 `Get` and 2 `Put`, plus 1 `GetRange` on fs;
  - moving one is 1 `Put`, plus 1 `GetRange` on fs;
  - resolving a name is 1 `Get`;
  - a bad name or record sends nothing.

  Tests: `an_alias_is_created_in_one_request_and_resolves`, `an_alias_and_an_index_cannot_share_a_name`, `an_alias_takes_over_a_deleted_indexs_tombstone`, `an_alias_moves_by_one_swap_from_the_generation_read`, `a_bad_name_or_record_sends_nothing`.
- AC-4: On each backend:
  - a name listing skips an alias it cannot decode, and lists every index;
  - an alias being created resolves as absent, with its generation, and `resolve_raw` shows it;
  - an alias with an update pending resolves to itself;
  - a stale move loses;
  - an index and an alias never share a name;
  - a data stream a Java node stored resolves, writes to its newest index, and writes back the Java's bytes.

  Tests: `an_alias_being_created_resolves_as_absent`, `an_alias_moves_by_one_swap_from_the_generation_read`, `an_alias_and_an_index_cannot_share_a_name`, `a_data_stream_a_java_node_stored_resolves`, `an_alias_is_not_an_index_and_holds_its_name`, `an_alias_is_skipped_without_being_decoded`.

## Test plan

- AC-1 and AC-2: `crates/format/tests/alias_records.rs` against `fixtures/aliases/`, written by the shell's own `AliasRecord` compiled against the OpenSearch 3.3.0 jars, and `crates/format/tests/records.rs`.
- AC-3 and AC-4: `crates/meta/tests/descriptors/aliases.rs` and `lifecycle.rs` over the three backends, with exact meter counts.

## S3 request budget

| Operation | PUT-class | GET-class | LIST | DELETE | Depth |
|---|---|---|---|---|---|
| `create_alias`, name never used | 1 | 0 | 0 | 0 | 1 |
| `create_alias`, over a tombstone | 2 | 1 (and 1 ranged on fs) | 0 | 0 | 3 |
| `update_alias` | 1 | 0 (1 ranged on fs) | 0 | 0 | 1 |
| `resolve` | 0 | 1 | 0 | 0 | 1 |

These are the Java's requests for the same calls. Aliases change at the rate of operator actions.

## Switch

None needed. Nothing calls the alias writes yet, and reading an alias register returns the same name as before, now decoded.

## Risks

- `NameRecord::parse` now refuses an alias register the Java would coerce, where it used to pass the bytes through. No node writes such a record. Resolving that name reports a malformed register; the name listing still skips it.

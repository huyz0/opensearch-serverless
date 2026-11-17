# Mappings: parse, merge and write back

## Serves

Parity plan R6.1, part 1 (roadmap stage 3, [roadmap](../../roadmap.md)): the writer reads an index's mapping, merges updates into it, and writes it in the form OpenSearch keeps, so indexing (R6.2) and dynamic mapping (R6.1, part 2) have a mapping to grow.

## Change

- `oss-writer` gains `mapping` (`crates/writer/src/mapping/`), a port of `MapperService.merge`, `DocumentMapperParser.parse`, `ObjectMapper`, `ParametrizedFieldMapper` and the `text`, `keyword`, numeric (`byte`, `short`, `integer`, `long`, `float`, `double`), `date` and `boolean` field mappers (server, OpenSearch 3.10 at `429b4cbc`):
  - `Mapping::parse` reads a create-index or put-mapping `mappings` body, or a descriptor's stored mapping, and refuses an invalid one with the Java's exception type, message and cause (`Failed to parse mapping [_doc]: ...`).
  - `Mapping::merge` merges an update: updateable parameters change, fixed ones must stay (`Mapper for [x] conflicts with existing mapper: ...`), a type never changes, objects merge child by child, multi-fields are added or merged; the total fields (1,000) and depth (20) limits hold after it.
  - `Mapping::to_json` writes `mappingSource()` without its `_doc` wrapper: children sorted by name in Java's `String` order, each parameter only when it differs from its default, `text` in its own order.
  - A mapping that is valid but uses something this writer does not index yet (another type, analyzer, normalizer, similarity, term vectors, `copy_to`, dynamic templates, `_source` or `_meta`, a date format other than the default, the slash formats and the epoch ones) is refused with `MappingError::unsupported`, so a router keeps that index on a Java writer.
  - Root options kept: `dynamic`, `date_detection`, `numeric_detection`, `_routing.required`.
- The shell stores a merged mapping re-serialized through a `HashMap`, so its member order is a hash order; two stored mappings are equal when they parse to the same `Mapping`, not when their text is.
- `fixtures/compat/mapping/` holds eight recorded fixtures (create, get and put mapping, dynamic mapping, refusals), recorded from a stock OpenSearch 3.10.0-SNAPSHOT built from `429b4cbc`.
- Not changed: dynamic mapping (part 2: the fixtures' document steps are replayed then), the descriptor CAS that stores a grown mapping (`MappingGrowthPath`, with indexing), the REST handlers (R7.4), the reader's own read-only `mapping.rs`.

## Acceptance criteria

- AC-1: every create-index, put-mapping and get-mapping step of `fixtures/compat/mapping/` before a document step replays: an accepted mapping is written back byte for byte as OpenSearch's `GET _mapping` shows it, and reads back to the same `Mapping`; a refused one is refused with the same exception type and message down its `caused_by` chain.
- AC-2: each listed unsupported type, parameter or root option is refused as unsupported, never as invalid; the same parameters at their default are accepted and not written.
- AC-3: a stored mapping with its members in any order parses to the same `Mapping` as its sorted form, and writes the sorted form.
- AC-4: 1,001 fields, or objects 21 deep, are refused with the Java's limit messages, at parse and at merge.
- AC-5: malformed JSON, a duplicate key and a non-object mapping are refused; `_routing.required` cannot change; `date_detection` and `numeric_detection` survive a merge that does not set them.

## Test plan

`crates/writer/tests/mapping_fixtures.rs` replays the recorded fixtures (AC-1); `crates/writer/tests/mapping.rs` covers AC-2 to AC-5. The recorded fixtures were failing tests before the code existed; `scripts/mutants.sh` on the diff stands in for watching each unit test fail.

## S3 request budget

Unchanged: parsing and merging read no blob. Storing a grown mapping is the descriptor CAS the shell already makes, priced with indexing.

## Switch

None: a mapping is either read as the Java reads it or refused as unsupported, and an unsupported index stays with a Java writer.

## Risks

- A parameter the Java writes that this port does not know turns a valid stored mapping into a refusal: every parameter the five mappers declare is in the tables, at its Java default (AC-1, AC-2).
- A default that differs from the Java's would write a parameter the Java leaves out, or drop one it keeps: the fixtures set each parameter on and off its default (AC-1).

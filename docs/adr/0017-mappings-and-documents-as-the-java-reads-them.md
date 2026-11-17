# 0017: Mappings and documents read as the Java reads them, and what the writer cannot index stays with a Java writer

**Status:** Accepted after independent review, 2026-11-17.

## Context

The writer (R6) indexes documents into the indexes the Java shell also writes and reads. Two Java behaviours decide what is stored:

- **The mapping.** Each index descriptor stores the mapping (`mappingSource`). The reader, the Java shell and `GET _mapping` all read it.
- **Dynamic mapping.** It decides which new fields a document adds, and with which types. A field mapped differently by the two writers would index differently, and the mapping change one writer stores would conflict with the other's.

OpenSearch maps about thirty field types, with analyzers, normalizers, dynamic templates and `copy_to`. The writer's first stage indexes five: `text`, `keyword`, the numeric types except `half_float`, `scaled_float` and `unsigned_long`, `date`, and `boolean`. Each has the standard analyzer and the default or epoch date formats.

The Java shell stores a merged mapping re-serialized through a `HashMap`, so its member order is a hash order. A Java-created mapping keeps the request's order instead.

## Decision

- **The writer reads a mapping and a document by porting the Java code paths, not by re-deriving them.** The ported paths are:
  - for mappings: `ParametrizedFieldMapper` parameters in `getParameters` order with their defaults, `ObjectMapper` merge, and `MapperService`'s limits;
  - for documents: `DocumentParser`'s descent, Jackson's number accessors and coercion rules, `Booleans.parseBoolean`, and the date formatters.
  - **Errors:** each refusal carries the Java's exception type and message down its `caused_by` chain.
  - **Correctness reference:** recorded fixtures from a stock OpenSearch built from the shell's base commit (`fixtures/compat/mapping/`). A difference from them is a bug, whichever side reads better.
- **A mapping that is valid but uses what the writer does not index is refused as `unsupported_mapping`, never accepted and ignored.** A parameter set to its default is not "used", so it is accepted and not written.
  - **No client sees `unsupported_mapping`.** It tells the routing layer to send that index's writes to a Java writer, the way the Java coordinator already sends unsupported searches to Java readers (roadmap, reader stage). The routing layer arrives with the coordinator (R7). Until then a Rust writer serves only indexes inside its subset, and the mixed-fleet gate (R6 exit) checks that.
  - **OpenSearch behaviour stays the same for every index:** an index outside the subset is written by the Java code that writes it today.
- **Stored mappings are compared by meaning.** Two descriptors' mappings are equal when they parse to the same model. The writer writes the canonical form: the form `GET _mapping` shows, children in Java `String` order, and each parameter only when it differs from its default.
- **Dynamic mapping follows the Java's rules, with no dynamic templates.** A document's lookups see only the mapping it started with. Each new value builds its own field, as `parseDynamicValue` does, and the fields merge into one update after the document is read. The update applies only when the document is accepted, so a value of another kind in the same array fails as the Java fails: `mapper [x] cannot be changed from type [long] to [text]`.
- **The grown mapping is stored by the descriptor compare-and-swap the shell's `MappingGrowthPath` already makes.** The swap is ported with indexing (R6.4), so this change has no code that writes it.
  - **Batching:** the additions a batch of writes brings are merged and swapped once.
  - **A lost swap:** re-read the descriptor (one GET), merge again, and swap again (one conditional PUT). This repeats at most 5 times, with jittered backoff, so two writers adding different fields both keep theirs.
  - **Cost:** requests scale with mapping changes, not documents. A document that adds no field adds no request.
- **A document is pulled token by token from its bytes** (`chunkjson`). Nothing is built from it but the values each field indexes and the update. The one exception is a refused value: the error path reads the rest of that value into a tree to preview it, as the Java does. That tree is bounded by the document's own bytes.
  - **The update is built in place as fields are added,** one insertion for each new field, so its cost is linear in the fields added.
  - **The total-fields limit (1,000) is checked as each field is added.** Past it, nothing more is added, and the document fails with the Java's limit message.

## Consequences

- **What gets easier:** an index the writer accepts is indexed as the Java would index it, and its stored mapping reads the same on both sides. Mixed fleets and moving an index between writers need no translation.
- **What gets harder:**
  - Every new type or parameter needs its Java defaults, merge rules and messages, and a recorded fixture before it is accepted.
  - Jackson's message texts, such as byte offsets in coercion errors and HashMap order in value previews, are reproduced and must be rechecked when the Java side upgrades Jackson.
- **Ruled out:**
  - indexing an approximation of an unsupported type;
  - accepting an unknown parameter;
  - a mapping store that keeps the request's text instead of the model.
- **Fixed limits for now:** the total-fields (1,000) and depth (20) limits are OpenSearch's defaults. An index that sets `index.mapping.total_fields.limit` or `index.mapping.depth.limit` is read with the defaults until index settings reach the writer, which comes with indexing (R6.2).
- **Costs:**
  - **S3:** no request is added beyond the shell's own descriptor update, which happens on a mapping change.
  - **Memory:** a mapping is bounded by its 8 MiB parse limit. A document's parse holds:
    - the document's bytes, which indexing keeps anyway as `_source`, up to the HTTP body bound (`http.max_content_length`, 100 MiB);
    - the values it indexes, at most about the document's bytes again, because each value is text or a number taken from it;
    - the update, at most 1,000 fields;
    - a path of at most 20 objects.

    A node's indexing pool reserves this, roughly twice the document's size, for each document in flight (ADR 0004). The reservation is made when indexing is wired (R6.2).

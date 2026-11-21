# 0023: A routed document stores its routing as a `_routing` field in the segment

**Status:** Accepted after independent review, 2026-11-21 (one round; the reviewer checked the claims below against the code and found none contradicted)

## Context

A get must return `_routing` for a document indexed with a custom routing value, as the Java engine does. The engine never kept it: the routing travelled on the write-log record (which already has a `routing` field) and decided the shard, and was then dropped, so a get, a forwarded get and the REST get could not return it. The forwarded-get decision (owner, 2026-11-21) asked for the engine to store and return it.

The routing has to survive what the document survives: a refresh, a publish, a replay of a predecessor's log, a takeover. Only the Lucene document does, and the Java stores `_routing` there (`RoutingFieldMapper.preParse`: one untokenized term, stored string), so a segment this engine writes keeps the Java's layout and a Java reader reading it finds the field it expects.

## Options

1. **Keep it in the version map.** Rejected: dropped at refresh, absent from a published commit, lost on takeover.
2. **A side object per shard.** Rejected: a new blob, a PUT per publish, and a second thing to fence.
3. **Store it in the Lucene document (chosen).** No new request: the field rides in the segment blob the shard already publishes.

## Decision

- `MetaField::Routing` (`_routing`, docs-only postings, no norms, stored string) is added to the document of a write that names a routing value, after `_id`. A document without one is built exactly as before.
- A routed write takes the complete ingest path; the fast plan compiles a fixed metadata layout and declines it.
- A get reads the stored field; `Fetched.routing`, `GetResponse.routing` and the REST get return it. A newer version without routing has none.
- Replay of a record keeps the record's routing.

## Consequences

- No new blob request: LIST 0, HEAD 0, PUT 0. A routed segment carries one more small stored field and term per routed document, bounded by the request's bytes.
- Documents that name no routing are unchanged byte for byte, so existing segments, fixtures and the fast path are untouched.
- A routed write is slower per document (the complete path), and only a write that names a routing value pays it. That downside is negligible and bounded to those writes, so there is no switch ([principles](../principles.md#switches-and-defaults)).
- Not decided here: searching on `_routing`, and the reader's overlay storing it.
- Not differentially tested against a Java-written segment for the field's position among the metadata fields; the field's name, postings and stored form follow the Java mapper.

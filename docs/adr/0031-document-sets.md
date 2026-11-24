# 0031: Templates and pipelines are document sets behind one marker register

**Status:** Accepted after independent review, 2026-11-24

## Context

Stock composable and component templates are how a schema is authored here, and ingest and search pipelines are operator configuration of the same kind. Core keeps them in cluster state; there is none. The Java shell keeps each kind in a container of immutable content blobs named by one marker register (`TemplateStore`), and a Rust node on the same bucket must read and change the same sets. They are few (tens, never millions) and read by name, and index creation must read every template to find the matching ones.

## Decision

- Each kind is a set under `templates/index/`, `templates/component/`, `pipelines/` or `search_pipelines/`: a marker register `_version` holding `{"version":n,"entries":{name: content}}`, and one blob `blob-<content>` per document. The bytes are the Java's.
- A change writes the document's bytes under a new key, then makes one compare-and-swap (or conditional create) of the marker, retried from a fresh read up to 10 times. The replaced bytes are deleted after the swap unless another name reads them; a blob no marker names is never read. No lock, no lease.
- Nothing lists. The marker holds the names, so a get is 2 GETs, a walk of the set is 1 plus one a document, and a put or delete is 1 GET, 1 or 2 PUTs and at most 1 DELETE. The count is capped at 1000 on the way in (a refusal on creating the 1001st), not by truncating an answer, so index creation can never fail for having too many to read.
- The form a store had before its marker held entries (a bare counter) is refused as unsupported, not migrated: a Java node carries it over on its first change.
- A marker's value for a name is the document's blob key (a 64-digit hash, `-` and a 22-character id), never the document: 1000 names of 255 bytes with their keys come to about 350 KB, so the bound is 384 KiB. A document is at most 1 MiB. Each read fits one caller buffer.
- A key is unique to one write, so no blob is shared by names a Rust node wrote; the delete after a swap reads no marker again and cannot drop bytes another name just took. Sharing exists only in the Java's carry-over of its old form, which is refused here.
- No switch: nothing calls the store yet. Wiring it into index creation needs a node cache of the set keyed by the marker's version first, and its own setting, and records the cap of 1000 as a difference from stock OpenSearch in `docs/compatibility.md`.

## Consequences

- Rust and Java nodes see the same templates and pipelines on one bucket, and a change is atomic: a reader that sees the new marker sees the new set.
- Index creation reads the whole set: 1 GET and one a document until the cache lands, bounded by the cap. That is why the wiring waits for the cache.
- A failed delete of replaced bytes, a crash after the bytes and before the swap, or a put that gives up under contention and cannot delete its bytes, leaves an unnamed blob that nothing sweeps (no LIST, and no repair path is planned): storage at the rate operators change templates, never a wrong read.
- Rules out a template or pipeline register per document, and any enumeration of a set that is not the marker.

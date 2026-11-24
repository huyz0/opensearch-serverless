# Derived `_source` for a `knn_vector` index, as the k-NN plugin stores it

## Serves

Parity plan R15, k-NN native, step 2c of four. 2b writes a vector into the segment and keeps it in `_source` too; the stock plugin (derived source is its default) stores `_source` with each vector replaced by the number `1` and puts the vector back when it reads. This spec makes the Rust writer store the same, so a Java node reads it and a segment is not twice the size.

## Change

- Recorded from stock OpenSearch 3.10.0 (a segment of `index.knn: true`, field `v`): the raw stored `_source` of a document with a vector is `{"v":1,"k":"a"}` (the value replaced by `1`, the key and every other byte kept); a document with no vector is stored whole; the segment's codec is `KNN1040Codec` and its `.si` has `derived_vector_fields=v` and `knn_delegate_stored_fields_codec_key=Lucene104`.
- Putting the vector back is lucene-rust's `SegmentReader::visit_source_document`, which splices it into a segment that lists derived fields and reads any other segment as stored. The search hits' fetch and the writer's realtime get read `_source` with the older `visit_stored_document`, which returns the masked `1`; they now use `visit_source_document`, so a segment a Java writer wrote (a knn index on a Java writer, read here) shows its vectors. The overlay reads only `_id` from a commit and is unchanged.
- Writing: a shard whose mapping has a `knn_vector` field opens its engine as the plugin's codec (`KNN1040Codec` over `Lucene104`, with the delegate attribute), lists each vector field under `derived_vector_fields` in every segment it flushes (and keeps the list across merges), and stores `_source` with each indexed vector's value replaced by `1`. A shard whose mapping has none opens as before.
- Masking is a splice: only the bytes of each vector's value change. A vector field is located by its full path in the source (`v`, or `o.v` inside object `o`); a source that names it in a form the splice cannot place (an escaped key, a dotted key at the top level) is refused as unsupported, so the router keeps it on a Java writer.
- No switch: it is the plugin's behaviour for every `index.knn` index, and a knn index is already behind `serverless.knn.enabled`.

## Acceptance criteria

- AC-1: the stock segments of the three `knn-*` fixtures (`fixtures/segments/files/`, the shard's files as the stock node wrote them, kept by `record.py --keep-files`) read as the plugin reads them: the raw stored `_source` of each of the six documents is the recorded one, and `visit_source_document` gives the vectors back (`[1.0,2.0,3.0]`, nested `o.v`, a numeric-string vector, a `null` left as it is).
- AC-2: the `_source` a shard stores for each document of the `knn-*` fixtures is byte for byte the raw stored `_source` of the stock segment, the document with no vector, the `null` one and the nested one included.
- AC-3: a shard over the knn engine writes a segment with codec `KNN1040Codec`, the two `.si` attributes, and reads each document's `_source` back with its vectors after a flush, and an update's new vector in a realtime get.
- AC-4: the plugin's own codec reads the Rust-written segments: `SegmentDump` with the k-NN plugin on the class path gives, for each of the three spaces, the `segment` of the stock fixture (fields, stored fields with the rebuilt `_source`, vectors, doc values), by hand. `CheckIndex` cannot run on a `KNN1040Codec` segment outside a node (the plugin's codec reads cluster settings), so it is not claimed here; 2b's `CheckIndex` is on the same vector files.
- AC-5: the splice leaves every other byte of the source alone for a vector at the top level and in a nested object, a vector after other keys, strings holding braces, brackets and escaped quotes; and refuses a source it cannot place.
- AC-6: an index with no `knn_vector` field writes the codec and `_source` it wrote before.

## S3 request budget

None added.

## Switch

None, as above.

## Known limits

A vector field added by a later mapping update is not listed in the segments of a shard opened before it, until the shard reopens; mapping updates are not routed to the writer yet. A merge keeps the list (lucene-rust unions it); no test here forces a merge. Search hits for a derived segment have no end-to-end test: the reader's fetch is the same call the writer's get tests.

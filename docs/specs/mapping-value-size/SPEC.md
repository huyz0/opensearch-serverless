# A mapping parameter value stays small

## Serves

[ADR 0054](../../adr/0054-parsed-mappings-are-shared-by-content.md), "Not decided here": a parsed mapping keeps about 1.4 KB a field, mostly because a leaf field stores one `Value` for every parameter its type declares, defaults included (up to 17), and `Value` was 96 bytes because its `Method` variant held a whole `KnnMethod` inline. A drifted index's own copy, which sharing cannot help, pays for it too.

## Change

`Value::Method` holds a `Box<KnnMethod>`. A `Value` is at most 40 bytes, so every leaf's parameter table is a third the size. Only a `knn_vector` field pays for the box. Storing only the parameters a mapping set is a further change, not made here.

## Acceptance criteria

- AC-1: `size_of::<Value>()` is at most 40 bytes (it was 96).
- AC-2: every existing mapping test passes unchanged: parse, write, merge and the `knn_vector` method round trip.

## S3 request budget

Zero.

## Switch

None: the same answers in less memory.

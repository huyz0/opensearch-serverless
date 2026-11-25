# Compiled search pipeline cache

## Serves

R14.6 follow-up to [search-pipelines](../search-pipelines/SPEC.md): a search that names a stored pipeline parsed its definition on every request. The node now keeps the compiled form of recently used small pipelines and reuses it while the stored bytes are unchanged.

## Change

- `SearchPipelines::resolve` still reads the set through the snapshot cache (one marker GET when unchanged, so a changed or removed pipeline is seen at once). After it has the stored bytes, a hit on the id whose byte length and keyed 64-bit hash equal the cached entry's returns a clone of the compiled pipeline; otherwise it compiles and, when the body is at most 16 KiB, stores the result.
- At most 16 entries; the oldest stored is dropped for a new one. A body over 16 KiB, or one that does not compile, is never cached. The most the cache holds is 16 compiled pipelines of bodies up to 16 KiB.
- The hash is SipHash with a per-process random key, so a stored body cannot be crafted to match another's.
- A removed pipeline answers missing as before (the snapshot no longer holds it), and its entry is replaced when the id is stored again.

## Acceptance criteria

- AC-1: Two searches naming one unchanged pipeline compile it once.
- AC-2: A replaced pipeline is compiled again and the new definition runs; a removed one is missing.
- AC-3: A body over 16 KiB is compiled on every resolve and not kept; the cache never holds more than 16 entries.
- AC-4: A resolve makes the same requests with the cache as without (rows `search_pipeline.resolve.requests`, unchanged).

## S3 request budget

Unchanged: the cache sits after the snapshot read.

## Switch

None: it saves work and makes no request; the entries are bounded as above (a fixed few hundred KiB at most), so no setting.

## Risks

The cache is not reserved from the memory budget: its bound is the constants above, small next to a search's own 2 MiB buffer reservation. The `filter_query` compile recorder of a pipeline that misses the cache is still limited to 1 MiB and unreserved, as before; a hit allocates none.

//! When the vector files of a shard are read in whole.
//!
//! The engine reads each file a vector search scans at random (`.vec`,
//! `.vex`, quantized codes, IVF vectors) whole when a segment opens, if it
//! is small next to the memory the process may use. On the object store that
//! is a ranged read of every byte, paid by the first search of an index even
//! when it reads no vector. With the open's warm off ([`set_at_open`]), a
//! search holding a `knn` query hands the files of every segment to one
//! prefetch ([`warm`]) before it walks any graph. See
//! [the spec](../../../docs/specs/reader-vector-warm/SPEC.md).

/// Whether a segment's vector files are read in whole when it opens (the
/// engine's own default), rather than at the first `knn` search. Process-wide,
/// because the engine's read advice is: set it once at start, before any shard opens (a shard warms by the value at its search, so changing it later mixes modes).
pub fn set_at_open(on: bool) {
    lucene_store::read_advice::set_warm_random(on);
}

/// Whether a segment's vector files are read in whole when it opens.
pub fn at_open() -> bool {
    lucene_store::read_advice::warm_random()
}

/// Reads in the vector files of every segment of `shard` in one batch, so a
/// `knn` search that follows finds them resident. A hint: a range the store
/// refuses is fetched again by the fault that needs it. Does nothing when
/// the files were read in at open ([`at_open`]) or are resident already.
pub(crate) fn warm<S: oss_blob::BlobStore + 'static>(shard: &crate::OpenShard<S>) {
    if at_open() {
        return;
    }
    let ranges: Vec<&[u8]> = shard
        .reader
        .segment_readers()
        .iter()
        .flat_map(|segment| segment.vector_warm_ranges())
        // ALLOC: one slice a vector file, a handful a segment.
        .collect();
    if ranges.is_empty() {
        return;
    }
    // Every segment's files at once: demand-filled files come in one
    // parallel batch.
    let _ = lucene_store::prefetch(&ranges);
    // Files the engine mapped from a spill file are advised, one by one.
    for range in ranges {
        lucene_store::read_ahead(range);
    }
}

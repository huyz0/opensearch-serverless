//! The commit of a shard that never held a document: one `segments_N` the
//! engine writes once for an empty index, served from memory.
//!
//! An index with a descriptor and no commit register is a created index
//! nobody wrote to. Stock OpenSearch searches it as an index of no
//! documents; this commit lets the reader do the same with no request past
//! the two it already makes.

use std::sync::OnceLock;

use lucene_index::index_writer::IndexWriter;
use lucene_index::segment_info::LuceneVersion;
use lucene_store::{ByteBuffersDirectory, Directory};

/// The blob key an inline file carries: no object has it.
pub(crate) const INLINE_KEY: &str = "<inline>";

const VERSION: LuceneVersion = LuceneVersion {
    major: 10,
    minor: 4,
    bugfix: 0,
};

/// The name and bytes of the empty commit's one file.
pub(crate) fn file() -> Option<&'static (String, Vec<u8>)> {
    static FILE: OnceLock<Option<(String, Vec<u8>)>> = OnceLock::new();
    FILE.get_or_init(build).as_ref()
}

fn build() -> Option<(String, Vec<u8>)> {
    // ALLOC: one directory and writer, built once a process.
    let dir = ByteBuffersDirectory::new();
    let mut writer = IndexWriter::open(&dir, Vec::new(), "Lucene104", VERSION).ok()?;
    writer.commit().ok()?;
    drop(writer);
    let name = dir
        .list_all()
        .ok()?
        .into_iter()
        .find(|n| n.starts_with("segments_"))?;
    // ALLOC: the empty commit's one file, a few hundred bytes, built once.
    let bytes = dir.open(&name).ok()?.as_ref().to_vec();
    Some((name, bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_empty_commit_is_one_segments_file_of_no_documents() {
        let (name, bytes) = file().expect("builds");
        assert!(name.starts_with("segments_"), "{name}");
        assert!(!bytes.is_empty());
    }
}

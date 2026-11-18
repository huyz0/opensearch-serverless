//! Shared by the overlay tests: a commit built in memory with `oss-doc`, so each
//! document has `_id`, `_seq_no` and its mapped fields as the writer writes them.

use lucene_index::buffered_updates::Term;
use lucene_index::index_writer::IndexWriter;
use lucene_index::segment_info::LuceneVersion;
use lucene_search::directory_reader::DirectoryReader;
use lucene_store::ByteBuffersDirectory;
use oss_doc::document;
use oss_doc::lucene::{Indexer, Meta};
use oss_doc::mapping::Mapping;

const VERSION: LuceneVersion = LuceneVersion {
    major: 10,
    minor: 4,
    bugfix: 0,
};

/// A commit with one segment for each batch of `(id, source, seq_no)`.
pub fn commit_of(
    mapping: &Mapping,
    batches: &[&[(&str, &str, i64)]],
) -> (ByteBuffersDirectory, DirectoryReader) {
    commit_without(mapping, batches, &[])
}

/// As [`commit_of`], with the documents of `deleted` ids removed by the commit itself.
pub fn commit_without(
    mapping: &Mapping,
    batches: &[&[(&str, &str, i64)]],
    deleted: &[&str],
) -> (ByteBuffersDirectory, DirectoryReader) {
    commit_with_data(mapping, batches, deleted, &[])
}

/// As [`commit_without`], the last commit carrying `user_data`.
pub fn commit_with_data(
    mapping: &Mapping,
    batches: &[&[(&str, &str, i64)]],
    deleted: &[&str],
    user_data: &[(&str, &str)],
) -> (ByteBuffersDirectory, DirectoryReader) {
    let dir = ByteBuffersDirectory::new();
    {
        let mut w = IndexWriter::open(&dir, Vec::new(), "Lucene104", VERSION).expect("writer");
        w.enable_explicit_documents().expect("explicit");
        let mut indexer = Indexer::new();
        for batch in batches {
            let mut docs = Vec::new();
            for (id, source, seq) in *batch {
                let parsed = document::parse(mapping, id, source.as_bytes()).expect("parse");
                let meta = Meta {
                    id,
                    seq_no: *seq,
                    primary_term: 1,
                    version: 1,
                    source: source.as_bytes(),
                };
                docs.push(
                    indexer
                        .document(&mut w, mapping, &parsed, &meta)
                        .expect("doc"),
                );
            }
            w.add_explicit_documents(docs).expect("add");
            w.commit().expect("commit");
        }
        if !deleted.is_empty() {
            let terms: Vec<Term> = deleted
                .iter()
                .map(|id| Term::new("_id", oss_doc::lucene::uid(id).expect("uid")))
                .collect();
            w.delete_documents_by_term(&terms).expect("delete");
            w.commit().expect("commit");
        }
        if !user_data.is_empty() {
            w.set_live_commit_data(
                user_data
                    .iter()
                    .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                    .collect(),
            );
            w.commit().expect("commit");
        }
    }
    let reader = DirectoryReader::open(&dir).expect("open");
    (dir, reader)
}

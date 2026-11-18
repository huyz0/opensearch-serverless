//! Shared doubles for the overlay builder tests: a commit a writer made, a
//! stage, records and a budget.

// Each test crate uses a different part of this module.
#![allow(dead_code)]
// ARITH: test sequence numbers are small constants.
#![allow(clippy::arithmetic_side_effects)]

use std::sync::Arc;

use lucene_index::index_writer::IndexWriter;
use lucene_index::segment_info::LuceneVersion;
use lucene_search::directory_reader::DirectoryReader;
use lucene_store::ByteBuffersDirectory;
use oss_mem::{BudgetConfig, MemoryBudget, Pool};
use oss_reader::commit_push::Stage;
use oss_reader::overlay::CommitAccess;
use oss_transport::actions::tail::Record;
use oss_writer::index::{Expect, Shard};
use oss_writer::mapping::Mapping;

pub const VERSION: LuceneVersion = LuceneVersion {
    major: 10,
    minor: 4,
    bugfix: 0,
};

pub const MAPPING: &str = r#"{"properties":{"n":{"type":"integer"}}}"#;

pub struct Commit {
    reader: DirectoryReader,
    _dir: ByteBuffersDirectory,
}

impl CommitAccess for Commit {
    fn reader(&self) -> &DirectoryReader {
        &self.reader
    }

    fn mapping_json(&self) -> &str {
        MAPPING
    }

    fn index_name(&self) -> &str {
        "logs"
    }
}

/// A commit a writer made of `ids`, with the checkpoint it stores.
pub fn commit(ids: &[&str]) -> Arc<dyn CommitAccess> {
    let dir = ByteBuffersDirectory::new();
    {
        let mut w = IndexWriter::open(&dir, Vec::new(), "Lucene104", VERSION).expect("writer");
        w.enable_explicit_documents().expect("explicit");
        let mapping = Mapping::parse(MAPPING.as_bytes()).expect("mapping");
        let mut shard = Shard::new(&dir, w, mapping, 3, Box::new(|| 0)).expect("shard");
        for id in ids {
            shard
                .index(id, br#"{"n":1}"#, false, &Expect::default())
                .expect("indexed");
        }
        shard.refresh().expect("refresh");
    }
    let reader = DirectoryReader::open(&dir).expect("opens");
    Arc::new(Commit { reader, _dir: dir })
}

pub fn stage(sequence: i64) -> Stage {
    Stage::new(3, Some("w1".to_owned()), sequence)
}

pub fn index(id: &str, n: i64, seq: i64) -> Record {
    Record {
        id: id.to_owned(),
        deletion: false,
        source: format!(r#"{{"n":{n}}}"#),
        seq_no: seq,
        primary_term: 1,
        version: seq + 1,
        routing: None,
    }
}

pub fn budget(open_indexes: usize) -> MemoryBudget {
    let config = BudgetConfig::new(64 << 20)
        .with_pool(Pool::OpenIndexes, open_indexes)
        .expect("fits");
    MemoryBudget::new(&config)
}

//! docs/specs/bulk-ingest: a run reads the pipeline set once and keeps what
//! it read for the request.

use std::sync::Arc;

use oss_blob::{BlobStore, Emulator};
use oss_format::DocKind;
use oss_meta::DocSet;
use oss_node::ingest_run::IngestPipelines;
use oss_writer::bulk::{Ingest, IngestError};

const BUFFER: usize = 1 << 16;

fn setup() -> (Arc<IngestPipelines>, DocSet) {
    let store: Arc<dyn BlobStore> = Arc::new(Emulator::new(1 << 24, 3));
    let set = IngestPipelines::open(&store, "", BUFFER).expect("opens");
    let docs = DocSet::new(store, "", DocKind::IngestPipeline).expect("set");
    (set, docs)
}

fn put(docs: &DocSet, id: &str, body: &str) {
    let mut marker = vec![0u8; BUFFER];
    docs.put(id, body.as_bytes(), false, &mut marker)
        .expect("stored");
}

#[test]
fn a_run_keeps_the_set_it_read_for_the_request() {
    let (set, docs) = setup();
    put(
        &docs,
        "p",
        r#"{"processors":[{"set":{"field":"n","value":7}}]}"#,
    );
    let mut run = set.run();
    assert_eq!(run.run("p", b"{}").unwrap(), br#"{"n":7}"#);
    // The pipeline changes and goes, and this request still sees its own.
    put(
        &docs,
        "p",
        r#"{"processors":[{"set":{"field":"n","value":8}}]}"#,
    );
    assert_eq!(run.run("p", b"{}").unwrap(), br#"{"n":7}"#);
    // The next request sees the change.
    assert_eq!(set.run().run("p", b"{}").unwrap(), br#"{"n":8}"#);
}

#[test]
fn a_missing_pipeline_and_a_failing_one_are_told_apart() {
    let (set, docs) = setup();
    put(
        &docs,
        "bad",
        r#"{"processors":[{"rename":{"field":"a","target_field":"b"}}]}"#,
    );
    let mut run = set.run();
    assert_eq!(run.run("none", b"{}"), Err(IngestError::Missing));
    assert!(matches!(run.run("bad", b"{}"), Err(IngestError::Failed(_))));
}

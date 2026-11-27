//! docs/specs/extension-analyzer AC-13: the shard path a coordinator
//! forwards (`search_shard_action::run`) parses a pinned field's text with
//! the module the index pins, and never with the built-in analyzer.
#![cfg(feature = "extensions")]

use lucene_store::ByteBuffersDirectory;
use oss_node::search_shard_action::run;
use oss_reader::dsl::parse_chunks;
use oss_testkit::TempDir;

use super::pinned_read::{module, modules, rig};

fn shard_search(r: &super::pinned_read::Rig) -> Result<u64, String> {
    let source =
        parse_chunks([br#"{"query":{"match":{"t":"zzzzzzz"}}}"#.as_slice()]).expect("parses");
    run(&r.service, "logs", 0, &source, &|_| Ok(()))
        .map(|found| found.total)
        .map_err(|e| format!("{e:?}"))
}

// AC-13 (the shard path)
#[test]
fn a_shard_search_analyses_with_the_module_the_index_pins() {
    let root = TempDir::new("pinned-shard");
    let dir = ByteBuffersDirectory::new();
    let r = rig(root.path(), &dir, &module());
    assert!(r.service.set_modules(modules(root.path())));
    assert_eq!(shard_search(&r), Ok(1));
}

// AC-13, AC-14 (the shard path never falls back)
#[test]
fn a_shard_search_with_no_module_loader_is_refused_and_never_answered_by_the_built_in() {
    let root = TempDir::new("pinned-shard-refused");
    let dir = ByteBuffersDirectory::new();
    let r = rig(root.path(), &dir, &module());
    let e = shard_search(&r).expect_err("refused");
    assert!(e.contains("extension_unavailable"), "{e}");
}

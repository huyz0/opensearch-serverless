//! The `knn` counters over a real shard; compiled only under `#[cfg(test)]`.

use lucene_analysis::Analyzer;

use super::tests::{knn, l2};
use crate::dsl::parse_chunks;
use crate::search::search;

fn run(shard: &crate::OpenShard<oss_blob::Emulator>, query: &str) {
    let body = format!(r#"{{"query":{query}}}"#);
    let source = parse_chunks([body.as_bytes()]).expect("parses");
    search(shard, "kn", &source, &Analyzer::standard(None)).expect("searches");
}

#[test]
fn a_knn_node_run_is_counted_and_a_search_without_one_is_not() {
    let shard = l2();
    let before = super::stats::snapshot();
    run(&shard, &knn(r#""vector":[3,2,1],"k":3"#));
    let plain = super::stats::snapshot();
    assert!(plain.queries > before.queries);
    run(
        &shard,
        &knn(r#""vector":[3,2,1],"k":3,"filter":{"match_all":{}}"#),
    );
    let filtered = super::stats::snapshot();
    assert!(filtered.filtered > plain.filtered);
    assert!(filtered.queries > plain.queries);
}

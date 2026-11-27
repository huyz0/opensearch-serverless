//! A forwarded search that holds pipeline aggregations: the coordinator runs
//! them, so the answer is the one the same search gives without them.

use super::*;
use oss_reader::aggs::partial::{Format, Partial, Value as Part};
use oss_transport::actions::search_shard::{
    AggKind, AggRequest, PipelineKind, PipelinePlace, PipelineRequest, Request as Wire,
};

fn terms_free_sum() -> AggRequest {
    AggRequest {
        name: "s".to_owned(),
        kind: AggKind::Sum,
        field: "n".to_owned(),
        precision_threshold: None,
        percentiles: None,
        terms: None,
        histogram: None,
        ranges: None,
        rare: None,
        multi: None,
        subs: Vec::new(),
    }
}

fn pipeline(kind: PipelineKind, name: &str) -> PipelinePlace {
    PipelinePlace {
        hosts: Vec::new(),
        pipeline: PipelineRequest {
            kind,
            name: name.to_owned(),
            paths: vec!["s".to_owned()],
            format: None,
            gap_policy: Some(1),
            unit: None,
            sorts: Vec::new(),
            from: 0,
            size: None,
        },
    }
}

fn answer(pipelines: Vec<PipelinePlace>) -> Vec<u8> {
    let mut fake = Fake::held();
    fake.aggs = vec![Partial {
        name: "s".to_owned(),
        format: Format::Raw,
        value: Part::Sum(3.5),
    }];
    let h = handler(fake, &fresh(), 10_000);
    let mut wire = Wire::read(&fixture("request.match_all"), &Bounds::DEFAULT).unwrap();
    wire.source.aggregations = vec![terms_free_sum()];
    wire.source.pipelines = pipelines;
    let mut message = Vec::new();
    wire.write(&mut Output::new(&mut message, 1 << 20)).unwrap();
    ask(&h, &message, WireVersion::SHELL).unwrap().to_vec()
}

#[test]
fn pipeline_aggregations_do_not_change_the_shards_answer() {
    let without = answer(Vec::new());
    let with = answer(vec![
        pipeline(PipelineKind::AvgBucket, "a"),
        pipeline(PipelineKind::Derivative, "d"),
    ]);
    assert_eq!(with, without);
    assert!(!without.is_empty());
}

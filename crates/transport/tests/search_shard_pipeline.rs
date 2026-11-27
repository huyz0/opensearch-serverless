//! The pipeline aggregations of a forwarded shard search against the bytes
//! OpenSearch 3.10.0 writes
//! (`fixtures/search-shard/java-pipeline-wire-3.10.0.txt`).

use oss_transport::actions::search_shard::*;
use oss_transport::stream::{Input, Output};

const RECORDED: &str = include_str!("../../../fixtures/search-shard/java-pipeline-wire-3.10.0.txt");

fn recorded(name: &str) -> Vec<u8> {
    let hex = RECORDED
        .lines()
        .find_map(|l| l.strip_prefix(name).and_then(|r| r.strip_prefix(' ')))
        .unwrap_or_else(|| panic!("no fixture {name}"));
    hex.as_bytes()
        .chunks(2)
        .map(|p| u8::from_str_radix(std::str::from_utf8(p).expect("ascii"), 16).expect("hex"))
        .collect()
}

/// Reads the recording, checks that it writes back as the same bytes, and
/// returns its pipeline aggregations.
fn read(name: &str) -> Vec<PipelinePlace> {
    let wire = recorded(name);
    let mut input = Input::new(&wire);
    let (aggs, places) = aggs::read_with_pipelines(&mut input).expect("reads");
    input.finish("aggregations").expect("all read");
    let mut buf = Vec::new();
    aggs::write_with_pipelines(&aggs, &places, &mut Output::new(&mut buf, 1 << 20))
        .expect("writes");
    assert_eq!(buf, wire, "{name} writes back as read");
    places
}

fn only(name: &str) -> PipelinePlace {
    let mut places = read(name);
    assert_eq!(places.len(), 1, "{name}");
    places.remove(0)
}

#[test]
fn the_sibling_kinds_read_from_java_bytes() {
    for (name, kind) in [
        ("avg_bucket", PipelineKind::AvgBucket),
        ("sum_bucket", PipelineKind::SumBucket),
        ("min_bucket", PipelineKind::MinBucket),
        ("max_bucket", PipelineKind::MaxBucket),
        ("stats_bucket", PipelineKind::StatsBucket),
    ] {
        let place = only(name);
        assert!(place.hosts.is_empty(), "{name} is at the top level");
        let p = place.pipeline;
        assert_eq!((p.kind, p.name.as_str()), (kind, "p"));
        assert_eq!(p.paths, ["t>s"]);
        assert_eq!((p.format, p.gap_policy), (None, Some(1)), "{name}: skip");
    }
}

#[test]
fn a_format_and_a_gap_policy_read_from_java_bytes() {
    let p = only("avg_bucket.options").pipeline;
    assert_eq!(p.format.as_deref(), Some("0.0"));
    assert_eq!(p.gap_policy, Some(0));
}

#[test]
fn a_derivative_reads_under_its_host() {
    let place = only("derivative");
    assert_eq!(place.hosts, ["h"]);
    let p = place.pipeline;
    assert_eq!(
        (p.kind, p.paths.as_slice()),
        (PipelineKind::Derivative, &["s".into()][..])
    );
    assert_eq!((p.format, p.gap_policy, p.unit), (None, Some(1), None));
    let p = only("derivative.options").pipeline;
    assert_eq!(p.format.as_deref(), Some("#.#"));
    assert_eq!((p.gap_policy, p.unit.as_deref()), (Some(0), Some("1d")));
}

#[test]
fn a_cumulative_sum_reads_from_java_bytes() {
    let p = only("cumulative_sum").pipeline;
    assert_eq!(
        (p.kind, p.format, p.gap_policy),
        (PipelineKind::CumulativeSum, None, None)
    );
    assert_eq!(
        only("cumulative_sum.format").pipeline.format.as_deref(),
        Some("0.00")
    );
}

#[test]
fn a_bucket_sort_reads_from_java_bytes() {
    let p = only("bucket_sort").pipeline;
    assert_eq!(p.kind, PipelineKind::BucketSort);
    assert_eq!(p.paths, ["s", "_key"]);
    assert_eq!(
        p.sorts,
        [
            SortField {
                field: "s".into(),
                descending: Some(true)
            },
            SortField {
                field: "_key".into(),
                descending: Some(false)
            },
        ]
    );
    assert_eq!((p.from, p.size, p.gap_policy), (1, Some(2), Some(1)));
    let p = only("bucket_sort.window").pipeline;
    assert!(p.sorts.is_empty() && p.paths.is_empty());
    assert_eq!((p.from, p.size, p.gap_policy), (0, Some(3), Some(0)));
}

#[test]
fn several_pipelines_keep_their_request_order() {
    let places = read("two");
    let names: Vec<_> = places.iter().map(|p| p.pipeline.name.as_str()).collect();
    assert_eq!(names, ["m", "a"]);
    assert_eq!(places[0].pipeline.kind, PipelineKind::MaxBucket);
    assert_eq!(places[1].pipeline.kind, PipelineKind::AvgBucket);
}

#[test]
fn a_reader_that_takes_no_pipelines_refuses_them_by_name() {
    let wire = recorded("avg_bucket");
    let refusal = aggs::read(&mut Input::new(&wire)).expect_err("refused");
    assert!(matches!(
        refusal,
        Refusal::Unsupported("pipeline aggregations")
    ));
}

/// An empty aggregation list and `n` pipeline aggregations, each written by
/// `one`.
fn builder(n: usize, one: impl Fn(&mut Output<'_>)) -> Vec<u8> {
    let mut buf = Vec::new();
    let mut out = Output::new(&mut buf, 1 << 20);
    out.vint(0).expect("aggs");
    out.vint(i32::try_from(n).expect("small")).expect("count");
    for _ in 0..n {
        one(&mut out);
    }
    buf
}

fn refused(wire: &[u8]) -> Refusal {
    aggs::read_with_pipelines(&mut Input::new(wire)).expect_err("refused")
}

fn avg_bucket_head(out: &mut Output<'_>) {
    out.string("avg_bucket").expect("type");
    out.string("p").expect("name");
    out.string_array(&["a"]).expect("paths");
    out.byte(0xFF).expect("metadata");
}

#[test]
fn another_pipeline_type_is_refused_by_name() {
    let wire = builder(1, |out| out.string("percentiles_bucket").expect("type"));
    assert!(matches!(
        refused(&wire),
        Refusal::Unsupported("a pipeline aggregation of another type")
    ));
}

#[test]
fn metadata_is_refused_by_name() {
    let wire = builder(1, |out| {
        out.string("avg_bucket").expect("type");
        out.string("p").expect("name");
        out.string_array(&["a"]).expect("paths");
        out.vint(0).expect("an empty map, not a null one");
    });
    assert!(matches!(
        refused(&wire),
        Refusal::Unsupported("pipeline aggregation metadata")
    ));
}

#[test]
fn a_sort_option_is_refused_by_name() {
    // A bucket_sort whose sort has a nested path.
    let wire = builder(1, |out| {
        out.string("bucket_sort").expect("type");
        out.string("b").expect("name");
        out.string_array(&["s"]).expect("paths");
        out.byte(0xFF).expect("metadata");
        out.vint(1).expect("one sort");
        out.string("s").expect("field");
        out.bool(false).expect("nested filter");
        out.bool(true).expect("nested path present");
    });
    assert!(matches!(
        refused(&wire),
        Refusal::Unsupported("a sort's nested path")
    ));
}

#[test]
fn a_gap_policy_that_is_not_one_is_malformed() {
    let wire = builder(1, |out| {
        avg_bucket_head(out);
        out.optional_string(None).expect("format");
        out.byte(7).expect("gap policy");
    });
    assert!(matches!(refused(&wire), Refusal::Wire(_)));
}

#[test]
fn more_than_sixty_four_pipelines_are_refused_before_any_is_read() {
    let wire = builder(65, avg_bucket_head);
    assert!(matches!(
        refused(&wire),
        Refusal::TooLarge("pipeline aggregations")
    ));
    // 64 of them read.
    let wire = builder(64, |out| {
        avg_bucket_head(out);
        out.optional_string(None).expect("format");
        out.byte(1).expect("gap policy");
    });
    assert_eq!(
        aggs::read_with_pipelines(&mut Input::new(&wire))
            .expect("reads")
            .1
            .len(),
        64
    );
}

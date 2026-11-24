//! docs/specs/node-assembly: the ingest pipeline routes and the write path's use of them.

#![cfg(all(feature = "reader", feature = "writer"))]

use oss_blob::BlobStore;
use oss_format::MAX_POINT_IN_TIME_BYTES;
use oss_node::assemble::Config;

use crate::assemble::{call, config, start, start_with};

#[test]
fn the_pipeline_routes_are_served_only_when_pipelines_are_on() {
    let (off, _) = start(256 << 20);
    assert_ne!(call(&off, "GET", "/_ingest/pipeline", None).status, 200);
    let on = Config {
        ingest_pipelines: true,
        ..config()
    };
    let (r, _) = start_with(on, 256 << 20);
    let body = r#"{"processors":[{"trim":{"field":"a"}}]}"#;
    assert_eq!(
        call(&r, "PUT", "/_ingest/pipeline/p", Some(body)).status,
        200
    );
    let got = call(&r, "GET", "/_ingest/pipeline/p", None);
    assert!(got.text().contains("trim"), "{}", got.text());
}

#[test]
fn a_write_runs_the_pipeline_it_names_only_when_pipelines_are_on() {
    let on = Config {
        ingest_pipelines: true,
        ..config()
    };
    let (r, store) = start_with(on, 256 << 20);
    store
        .create_register(
            "indices/docs",
            br#"{"name":"docs","uuid":"docs-uuid","number_of_shards":1,"mapping":"{\"properties\":{\"n\":{\"type\":\"integer\"}}}","settings":{}}"#,
        )
        .expect("descriptor");
    let made = call(
        &r,
        "PUT",
        "/_ingest/pipeline/seven",
        Some(r#"{"processors":[{"set":{"field":"n","value":7}}]}"#),
    );
    assert_eq!(made.status, 200, "{}", made.text());
    let bulk = concat!(
        "{\"index\":{\"_id\":\"1\",\"pipeline\":\"seven\"}}\n{\"n\":1}\n",
        "{\"index\":{\"_id\":\"2\",\"pipeline\":\"nope\"}}\n{\"n\":2}\n",
        "{\"index\":{\"_id\":\"3\"}}\n{\"n\":3}\n",
    );
    let put = call(&r, "POST", "/docs/_bulk", Some(bulk));
    assert_eq!(put.status, 200, "{}", put.text());
    assert!(
        put.text()
            .contains("pipeline with id [nope] does not exist"),
        "{}",
        put.text()
    );
    let one = call(&r, "GET", "/docs/_doc/1", None);
    assert!(one.text().contains("\"n\":7"), "{}", one.text());
    assert!(call(&r, "GET", "/docs/_doc/2", None).status == 404);
    let three = call(&r, "GET", "/docs/_doc/3", None);
    assert!(three.text().contains("\"n\":3"), "{}", three.text());
    let single = call(&r, "PUT", "/docs/_doc/4?pipeline=seven", Some(r#"{"n":4}"#));
    assert_eq!(single.status, 201, "{}", single.text());
    let four = call(&r, "GET", "/docs/_doc/4", None);
    assert!(four.text().contains("\"n\":7"), "{}", four.text());

    let (off, _) = start(256 << 20);
    let refused = call(
        &off,
        "PUT",
        "/docs/_doc/5?pipeline=seven",
        Some(r#"{"n":5}"#),
    );
    assert_eq!(refused.status, 400, "{}", refused.text());
}

#[test]
fn a_write_that_may_name_a_pipeline_reserves_the_run_s_buffers() {
    let on = Config {
        ingest_pipelines: true,
        ..config()
    };
    // A pool that cannot hold the run's two buffers refuses a write that names a pipeline.
    let (r, _) = start_with(on, MAX_POINT_IN_TIME_BYTES);
    let bulk = call(
        &r,
        "POST",
        "/docs/_bulk",
        Some("{\"index\":{\"_id\":\"1\"}}\n{\"n\":1}\n"),
    );
    assert_eq!(bulk.status, 429, "{}", bulk.text());
    let single = call(&r, "PUT", "/docs/_doc/2?pipeline=p", Some(r#"{"n":2}"#));
    assert_eq!(single.status, 429, "{}", single.text());
}

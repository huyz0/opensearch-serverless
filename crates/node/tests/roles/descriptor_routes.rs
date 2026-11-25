//! docs/specs/node-descriptor-routes: a writer finds its indexes in the
//! descriptors.

use std::sync::Arc;

use oss_blob::{BlobStore, Emulator, S3Op};
use oss_format::{AliasRecord, IndexRecord};
use oss_meta::{DeleteMode, DescriptorStore};
use oss_node::descriptor_routes::DescriptorRoutes;
use oss_node::writer::Routes;
use oss_writer::bulk::{Action, IndexInfo, ItemResult, Kind, Op, Owner, Planned};

const BUF: usize = 1 << 16;

fn rig() -> (Arc<Emulator>, DescriptorStore, DescriptorRoutes) {
    let store = Arc::new(Emulator::new(1 << 26, 5));
    let make = |s: &Arc<Emulator>| {
        DescriptorStore::new(
            Arc::clone(s) as Arc<dyn BlobStore>,
            "",
            DeleteMode::Conditional,
        )
        .expect("names")
    };
    let routes = DescriptorRoutes::new(make(&store), BUF);
    let descriptors = make(&store);
    (store, descriptors, routes)
}

fn put(d: &DescriptorStore, name: &str, shards: u32, settings: &str, mapping: Option<&str>) {
    let mut record = IndexRecord::new(name, "uuid-1", shards);
    record.settings = settings.as_bytes();
    record.mapping = mapping.map(std::borrow::Cow::Borrowed);
    d.create(&record, 1, &mut vec![0u8; BUF]).expect("created");
}

#[test]
fn an_index_created_without_settings_has_the_routing_defaults() {
    let (_, d, routes) = rig();
    put(&d, "logs", 3, "{}", None);
    let info = routes.index("logs").expect("found");
    assert_eq!(
        info,
        IndexInfo {
            name: "logs".into(),
            uuid: "uuid-1".into(),
            shards: 3,
            partition_size: 1,
            routing_shards: 3,
            routing_required: false,
            default_pipeline: None,
            final_pipeline: None,
        }
    );
}

#[test]
fn routing_settings_and_the_mapping_are_read_flat_nested_numeric_or_string() {
    let (_, d, routes) = rig();
    put(
        &d,
        "flat",
        4,
        r#"{"index.routing_partition_size":"2","index.number_of_routing_shards":8}"#,
        Some(r#"{"_routing":{"required":true}}"#),
    );
    let info = routes.index("flat").expect("found");
    assert_eq!((info.partition_size, info.routing_shards), (2, 8));
    assert!(info.routing_required);
    put(
        &d,
        "nested",
        4,
        r#"{"index":{"routing_partition_size":2,"number_of_routing_shards":"16"}}"#,
        None,
    );
    let info = routes.index("nested").expect("found");
    assert_eq!((info.partition_size, info.routing_shards), (2, 16));
    assert!(!info.routing_required);
    put(
        &d,
        "junk",
        4,
        r#"{"index.routing_partition_size":"x"}"#,
        None,
    );
    assert_eq!(routes.index("junk").expect("found").partition_size, 1);
}

#[test]
fn an_alias_an_absent_name_and_a_malformed_register_are_no_index() {
    let (store, d, routes) = rig();
    put(&d, "logs", 1, "{}", None);
    d.create_alias(&AliasRecord::new("a", &["logs"]), 1, &mut vec![0u8; BUF])
        .expect("alias");
    assert!(routes.index("a").is_none());
    assert!(routes.index("nothing").is_none());
    store
        .create_register("indices/broken", b"not a descriptor")
        .expect("planted");
    assert!(routes.index("broken").is_none());
}

#[test]
fn a_lookup_is_one_read_and_no_list() {
    let (store, d, routes) = rig();
    put(&d, "logs", 1, "{}", None);
    let count = |op| store.meter().count(op);
    let (gets, lists) = (count(S3Op::Get), count(S3Op::List));
    let before: u64 = S3Op::ALL.iter().map(|op| count(*op)).sum();
    assert!(routes.index("logs").is_some());
    let after: u64 = S3Op::ALL.iter().map(|op| count(*op)).sum();
    assert_eq!(after - before, 1);
    assert_eq!(count(S3Op::List), lists);
    assert!(count(S3Op::Get) >= gets);
}

#[test]
fn every_shard_is_local_and_a_forward_fails_each_item() {
    let (_, d, routes) = rig();
    put(&d, "logs", 2, "{}", None);
    let info = routes.index("logs").expect("found");
    assert_eq!(routes.owner(&info, 0), Owner::Local);
    assert_eq!(routes.owner(&info, 1), Owner::Local);
    let op = Op {
        action: Action {
            kind: Kind::Index,
            index: Some("logs".into()),
            id: Some("1".into()),
            routing: None,
            create: false,
            expect: Default::default(),
            pipeline: None,
            require_alias: false,
        },
        source: b"{}".to_vec(),
    };
    let group = [
        Planned {
            position: 0,
            op: &op,
            id: "1",
        },
        Planned {
            position: 1,
            op: &op,
            id: "2",
        },
    ];
    let results = routes.forward("other", &info, 1, &group);
    assert_eq!(results.len(), 2);
    for r in results {
        match r {
            ItemResult::Failed { status, error } => {
                assert_eq!(status, 503);
                assert_eq!(error.kind(), "no_owner");
            }
            other => panic!("{other:?}"),
        }
    }
}

// docs/specs/index-default-pipelines AC-4
#[test]
fn the_routes_read_the_default_and_final_pipelines() {
    let (_, d, routes) = rig();
    put(
        &d,
        "flat",
        1,
        r#"{"index.default_pipeline":"a","index.final_pipeline":"b"}"#,
        None,
    );
    let info = routes.index("flat").expect("found");
    assert_eq!(info.default_pipeline.as_deref(), Some("a"));
    assert_eq!(info.final_pipeline.as_deref(), Some("b"));
    put(
        &d,
        "nested",
        1,
        r#"{"index":{"default_pipeline":"c"}}"#,
        None,
    );
    let info = routes.index("nested").expect("found");
    assert_eq!(info.default_pipeline.as_deref(), Some("c"));
    assert_eq!(info.final_pipeline, None);
    put(&d, "empty", 1, r#"{"index.default_pipeline":""}"#, None);
    assert_eq!(routes.index("empty").expect("found").default_pipeline, None);
    put(&d, "none", 1, "{}", None);
    assert_eq!(routes.index("none").expect("found").final_pipeline, None);
}

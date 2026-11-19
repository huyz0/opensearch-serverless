//! docs/specs/coordinator-bulk-route: each bulk item is routed to a shard
//! and items are grouped by shard.

// ARITH: test counts are small constants.
#![allow(clippy::arithmetic_side_effects)]

use std::cell::RefCell;
use std::collections::HashMap;

use oss_coordinator::docs::bulk::{
    Group, ItemFailure, ItemInput, Routed, Target, Targets, group_by_shard, route_items,
};
use oss_coordinator::route::place::PlaceError;

struct Fake {
    targets: HashMap<String, Target>,
    broken: Vec<String>,
    lookups: RefCell<Vec<String>>,
}

impl Targets for Fake {
    fn is_system_index(&self, name: &str) -> bool {
        name == ".sec"
    }
    fn write_target(&self, name: &str) -> Result<Option<Target>, PlaceError> {
        self.lookups.borrow_mut().push(name.to_owned());
        if self.broken.iter().any(|b| b == name) {
            return Err(PlaceError("down".into()));
        }
        Ok(self.targets.get(name).cloned())
    }
    fn shard_for(&self, _: &Target, id: &str, routing: Option<&str>) -> Result<u32, String> {
        if id == "needs-routing" && routing.is_none() {
            return Err("routing is required for [logs]/[needs-routing]".into());
        }
        Ok(u32::try_from(id.len() % 3).expect("small"))
    }
}

fn fake() -> Fake {
    let mut targets = HashMap::new();
    for name in ["logs", "metrics"] {
        targets.insert(
            name.to_owned(),
            Target {
                index: name.to_owned(),
                uuid: format!("{name}-uuid"),
                data_stream: false,
            },
        );
    }
    targets.insert(
        "stream".to_owned(),
        Target {
            index: ".ds-stream-1".to_owned(),
            uuid: "ds-uuid".to_owned(),
            data_stream: true,
        },
    );
    Fake {
        targets,
        broken: vec!["flaky".to_owned()],
        lookups: RefCell::new(Vec::new()),
    }
}

fn item(index: Option<&str>, id: &str) -> ItemInput {
    ItemInput {
        index: index.map(str::to_owned),
        id: id.to_owned(),
        routing: None,
        failed: false,
        create: false,
        deletion: false,
    }
}

fn failure(status: u16, kind: &str, reason: &str) -> ItemFailure {
    ItemFailure {
        status,
        kind: kind.to_owned(),
        reason: reason.to_owned(),
    }
}

#[test]
fn an_item_already_failed_is_not_looked_up() {
    let plane = fake();
    let mut i = item(Some("logs"), "a");
    i.failed = true;
    let out = route_items(&plane, &[i]);
    assert_eq!(out, vec![None]);
    assert!(plane.lookups.borrow().is_empty());
}

#[test]
fn a_missing_or_blank_index_fails_the_item() {
    let plane = fake();
    let out = route_items(&plane, &[item(None, "a"), item(Some("  "), "b")]);
    let want = Some(Err(failure(
        400,
        "missing_index",
        "no index given for this item and none in the path",
    )));
    assert_eq!(out, vec![want.clone(), want]);
}

#[test]
fn a_system_index_is_refused_before_any_lookup() {
    let plane = fake();
    let out = route_items(&plane, &[item(Some(".sec"), "a")]);
    assert_eq!(
        out,
        vec![Some(Err(failure(
            403,
            "system_index",
            "[.sec] belongs to a plugin and is not reachable through the request path"
        )))]
    );
    assert!(plane.lookups.borrow().is_empty());
}

#[test]
fn lookups_fail_per_index_and_happen_once_per_name() {
    let plane = fake();
    let items = [
        item(Some("flaky"), "a"),
        item(Some("flaky"), "b"),
        item(Some("gone"), "c"),
        item(Some("gone"), "d"),
        item(Some("logs"), "e"),
        item(Some("logs"), "f"),
    ];
    let out = route_items(&plane, &items);
    let flaky = Some(Err(failure(
        503,
        "metadata_unavailable",
        "could not resolve index [flaky]; retry",
    )));
    let gone = Some(Err(failure(404, "index_not_found", "no such index: gone")));
    assert_eq!(out[0], flaky);
    assert_eq!(out[1], flaky);
    assert_eq!(out[2], gone);
    assert_eq!(out[3], gone);
    assert!(matches!(out[4], Some(Ok(_))));
    assert_eq!(*plane.lookups.borrow(), ["flaky", "gone", "logs"]);
}

#[test]
fn a_data_stream_takes_only_creates() {
    let plane = fake();
    let mut create = item(Some("stream"), "a");
    create.create = true;
    let mut delete = item(Some("stream"), "b");
    delete.create = true;
    delete.deletion = true;
    let plain = item(Some("stream"), "c");
    let out = route_items(&plane, &[create, delete, plain]);
    assert!(matches!(out[0], Some(Ok(_))));
    let refused = Some(Err(failure(
        400,
        "illegal_argument_exception",
        "only write ops with an op_type of create are allowed in data streams; \
         address the backing index [.ds-stream-1] for anything else",
    )));
    assert_eq!(out[1], refused);
    assert_eq!(out[2], refused);
}

#[test]
fn a_routed_item_names_its_backing_index_uuid_and_shard() {
    let plane = fake();
    let mut stream = item(Some("stream"), "abcd");
    stream.create = true;
    let out = route_items(
        &plane,
        &[
            item(Some("logs"), "ab"),
            stream,
            item(Some("logs"), "needs-routing"),
        ],
    );
    assert_eq!(
        out[0],
        Some(Ok(Routed {
            index: "logs".into(),
            uuid: "logs-uuid".into(),
            shard: 2,
        }))
    );
    assert_eq!(
        out[1],
        Some(Ok(Routed {
            index: ".ds-stream-1".into(),
            uuid: "ds-uuid".into(),
            shard: 1,
        }))
    );
    assert_eq!(
        out[2],
        Some(Err(failure(
            400,
            "routing_missing_exception",
            "routing is required for [logs]/[needs-routing]"
        )))
    );
}

#[test]
fn items_group_by_index_and_shard_in_first_seen_order() {
    let r = |index: &str, shard: u32| {
        Some(Ok(Routed {
            index: index.into(),
            uuid: format!("{index}-uuid"),
            shard,
        }))
    };
    let routed = vec![
        r("logs", 1),
        r("metrics", 0),
        None,
        r("logs", 1),
        Some(Err(failure(400, "x", "y"))),
        r("logs", 0),
        r("metrics", 0),
    ];
    let groups = group_by_shard(&routed);
    assert_eq!(
        groups,
        vec![
            Group {
                index: "logs".into(),
                uuid: "logs-uuid".into(),
                shard: 1,
                items: vec![0, 3],
            },
            Group {
                index: "metrics".into(),
                uuid: "metrics-uuid".into(),
                shard: 0,
                items: vec![1, 6],
            },
            Group {
                index: "logs".into(),
                uuid: "logs-uuid".into(),
                shard: 0,
                items: vec![5],
            },
        ]
    );
}

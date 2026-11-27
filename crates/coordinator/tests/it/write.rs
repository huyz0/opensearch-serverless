//! docs/specs/coordinator-write: where a write is applied and which native
//! write options it accepts.

// ARITH: test counts are small constants.
#![allow(clippy::arithmetic_side_effects)]

use std::sync::Mutex;

use oss_coordinator::docs::write::{
    ActiveShards, VersionKind, WritePlan, plan_write, settle, validate_native_write,
};
use oss_coordinator::route::place::{PlaceError, Placement, Plane, Refusal, Router};

struct Fake {
    doubts: Mutex<Vec<(String, u32)>>,
}

impl Plane for Fake {
    type Desc = ();
    fn is_system_index(&self, _: &str) -> bool {
        false
    }
    fn describe(&self, _: &str) -> Result<Option<()>, PlaceError> {
        Ok(Some(()))
    }
    fn uuid(&self, (): &()) -> String {
        "u1".into()
    }
    fn shard_for(&self, (): &(), _: &str, _: Option<&str>) -> u32 {
        0
    }
    fn owner_hint(&self, _: &str, _: u32) -> Option<String> {
        None
    }
    fn read_head(&self, _: &str, _: u32) -> Result<Option<String>, PlaceError> {
        Ok(None)
    }
    fn forget_owner(&self, _: &str, _: u32) {}
    fn holds_shard(&self, _: &str, _: &str, _: u32) -> bool {
        false
    }
    fn local_node_id(&self) -> &'static str {
        "me"
    }
    fn ownership_doubted(&self, index: &str, shard: u32) {
        self.doubts
            .lock()
            .expect("lock")
            .push((index.into(), shard));
    }
}

fn fake() -> Fake {
    Fake {
        doubts: Mutex::default(),
    }
}

fn at(local: bool, owner: Option<&str>, uuid: &str) -> Placement {
    Placement {
        local,
        shard: 4,
        owner: owner.map(str::to_owned),
        uuid: uuid.into(),
    }
}

fn plan(plane: &Fake, p: &Placement, activated: bool) -> Result<WritePlan, Refusal> {
    plan_write(&Router::new(plane, false), "logs", p, "me", activated)
}

#[test]
fn a_held_shard_is_written_here_whoever_the_head_names() {
    for owner in [None, Some("me"), Some("n9")] {
        assert_eq!(
            plan(&fake(), &at(true, owner, "u1"), false),
            Ok(WritePlan::Local)
        );
    }
}

#[test]
fn an_unheld_shard_nobody_or_this_node_owns_is_activated_first() {
    for owner in [None, Some("me")] {
        assert_eq!(
            plan(&fake(), &at(false, owner, "u1"), false),
            Ok(WritePlan::Activate)
        );
    }
}

#[test]
fn after_the_activation_was_tried_the_refusal_says_why() {
    let plane = fake();
    assert_eq!(
        plan(&plane, &at(false, None, "u1"), true),
        Err(Refusal::NotHere {
            message: "no node currently owns shard 4 of logs".into(),
            owner: None,
            retryable: false,
        })
    );
    assert_eq!(
        plan(&plane, &at(false, Some("me"), "u1"), true),
        Err(Refusal::NotHere {
            message: "this node is acquiring shard 4 of logs; retry".into(),
            owner: Some("me".into()),
            retryable: true,
        })
    );
    assert_eq!(plane.doubts.lock().expect("lock").len(), 2);
}

#[test]
fn another_nodes_shard_is_forwarded_without_activating() {
    for activated in [false, true] {
        assert_eq!(
            plan(&fake(), &at(false, Some("n2"), "u1"), activated),
            Ok(WritePlan::Forward { owner: "n2".into() })
        );
    }
}

#[test]
fn only_the_same_incarnation_settles_after_an_activation() {
    let before = at(false, None, "u1");
    let same = at(true, Some("me"), "u1");
    let other = at(true, Some("me"), "u2");
    assert_eq!(settle(&before, same.clone()), same);
    assert_eq!(settle(&before, other), before);
}

#[test]
fn native_writes_accept_only_what_the_log_can_honour() {
    let ok = ActiveShards::Count(1);
    assert_eq!(
        validate_native_write(-3, VersionKind::Internal, &ok, false),
        Ok(())
    );
    assert_eq!(
        validate_native_write(-4, VersionKind::Internal, &ok, true),
        Ok(())
    );
    let version = Err("version/version_type is not supported; use if_seq_no and if_primary_term");
    assert_eq!(
        validate_native_write(-4, VersionKind::Internal, &ok, false),
        version
    );
    assert_eq!(
        validate_native_write(7, VersionKind::Internal, &ok, true),
        version
    );
    assert_eq!(
        validate_native_write(-3, VersionKind::External, &ok, false),
        version
    );
    assert_eq!(
        validate_native_write(-3, VersionKind::ExternalGte, &ok, false),
        version
    );
    let shards = Err("wait_for_active_shards cannot exceed one: shards have no replicas");
    for fine in [
        ActiveShards::Default,
        ActiveShards::All,
        ActiveShards::Count(0),
    ] {
        assert_eq!(
            validate_native_write(-3, VersionKind::Internal, &fine, false),
            Ok(())
        );
    }
    assert_eq!(
        validate_native_write(-3, VersionKind::Internal, &ActiveShards::Count(2), false),
        shards
    );
    // The version is checked first.
    assert_eq!(
        validate_native_write(5, VersionKind::Internal, &ActiveShards::Count(2), false),
        version
    );
}

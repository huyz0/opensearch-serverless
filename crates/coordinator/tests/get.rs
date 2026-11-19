//! docs/specs/coordinator-get: where a get is served and whether its version holds.

// ARITH: test counts are small constants.
#![allow(clippy::arithmetic_side_effects)]

use std::cell::Cell;
use std::sync::Mutex;

use oss_coordinator::docs::get::{
    GetPlan, MATCH_ANY, MATCH_DELETED, NOT_FOUND, plan_get, version_conflict,
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

fn at(local: bool, owner: Option<&str>) -> Placement {
    Placement {
        local,
        shard: 2,
        owner: owner.map(str::to_owned),
        uuid: "u1".into(),
    }
}

fn plan(
    plane: &Fake,
    placement: &Placement,
    published: Result<bool, PlaceError>,
) -> (Result<GetPlan, Refusal>, u32) {
    let router = Router::new(plane, false);
    let lookups = Cell::new(0);
    let out = plan_get(&router, "logs", placement, "me", || {
        lookups.set(lookups.get() + 1);
        published
    });
    (out, lookups.get())
}

fn fake() -> Fake {
    Fake {
        doubts: Mutex::default(),
    }
}

#[test]
fn a_shard_held_here_and_owned_here_or_by_nobody_is_read_here() {
    for owner in [None, Some("me")] {
        let (out, lookups) = plan(&fake(), &at(true, owner), Ok(true));
        assert_eq!(out, Ok(GetPlan::Local));
        assert_eq!(lookups, 0);
    }
}

#[test]
fn a_shard_held_here_but_owned_elsewhere_goes_to_its_owner() {
    let (out, lookups) = plan(&fake(), &at(true, Some("n7")), Ok(true));
    assert_eq!(out, Ok(GetPlan::Forward { owner: "n7".into() }));
    assert_eq!(lookups, 0);
}

#[test]
fn with_no_owner_nothing_published_is_absent_and_a_commit_is_read() {
    let (out, lookups) = plan(&fake(), &at(false, None), Ok(false));
    assert_eq!(out, Ok(GetPlan::Absent));
    assert_eq!(lookups, 1);
    let (out, _) = plan(&fake(), &at(false, None), Ok(true));
    assert_eq!(out, Ok(GetPlan::Reader));
    let (out, _) = plan(&fake(), &at(false, None), Err(PlaceError("boom".into())));
    assert_eq!(out, Err(Refusal::Metadata("boom".into())));
}

#[test]
fn another_nodes_shard_is_forwarded_to_it() {
    let (out, lookups) = plan(&fake(), &at(false, Some("n3")), Ok(true));
    assert_eq!(out, Ok(GetPlan::Forward { owner: "n3".into() }));
    assert_eq!(lookups, 0);
}

#[test]
fn a_shard_owned_here_but_not_held_is_refused_as_acquiring() {
    let plane = fake();
    let (out, _) = plan(&plane, &at(false, Some("me")), Ok(true));
    assert_eq!(
        out,
        Err(Refusal::NotHere {
            message: "this node is acquiring shard 2 of logs; retry".into(),
            owner: Some("me".into()),
            retryable: true,
        })
    );
    assert_eq!(
        *plane.doubts.lock().expect("lock"),
        vec![("logs".to_owned(), 2)]
    );
}

#[test]
fn a_pinned_version_conflicts_as_the_shell_says() {
    assert_eq!(version_conflict(5, MATCH_ANY), None);
    assert_eq!(version_conflict(5, 5), None);
    assert_eq!(
        version_conflict(5, 4),
        Some("current version [5] is different than the one provided [4]".into())
    );
    assert_eq!(
        version_conflict(5, MATCH_DELETED),
        Some("current version [5] is different than the one provided [-4]".into())
    );
    assert_eq!(
        version_conflict(NOT_FOUND, 3),
        Some("document does not exist (expected version [3])".into())
    );
}

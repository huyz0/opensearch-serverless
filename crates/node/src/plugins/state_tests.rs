//! The state operations over a metered emulator: round trips, isolation
//! between plugins and tenants, bounds, and the request count of each call.

use std::sync::Arc;

use oss_blob::{BlobStore, Emulator};
use oss_ext::host::{Backend, HostError, NoBackend, Who};
use oss_ext::state::{Put, decode_found, encode_key, encode_put};
use oss_mem::{BudgetConfig, MemoryBudget, Pool};

use super::StateBackend;

struct Rig {
    backend: StateBackend,
}

fn rig_with(request_bodies: usize) -> Rig {
    let store = Arc::new(Emulator::new(1 << 30, 3)) as Arc<dyn BlobStore>;
    let budget = MemoryBudget::new(
        &BudgetConfig::new(256 << 20)
            .with_pool(Pool::RequestBodies, request_bodies)
            .expect("fits"),
    );
    let backend = StateBackend::new(store, "", &budget, Arc::new(NoBackend));
    Rig { backend }
}

fn rig() -> Rig {
    rig_with(64 << 20)
}

fn who<'a>(plugin: &'a str, tenant: &'a str) -> Who<'a> {
    Who {
        plugin,
        tenant,
        grants: &[],
    }
}

fn put(
    r: &Rig,
    w: &Who<'_>,
    key: &str,
    value: &[u8],
    if_absent: bool,
) -> Result<Vec<u8>, HostError> {
    let p = Put {
        key: key.to_owned(),
        value: value.to_vec(),
        if_absent,
    };
    r.backend.call(w, "state.put", &encode_put(&p))
}

fn get(r: &Rig, w: &Who<'_>, key: &str) -> Option<Vec<u8>> {
    let out = r
        .backend
        .call(w, "state.get", &encode_key(key))
        .expect("get");
    decode_found(&out).expect("answer")
}

#[test]
fn a_value_put_is_read_back_and_a_missing_key_is_none() {
    let r = rig();
    let w = who("geo", "t1");
    assert_eq!(get(&r, &w, "k"), None);
    assert_eq!(put(&r, &w, "k", b"v1", false), Ok(vec![1]));
    assert_eq!(get(&r, &w, "k"), Some(b"v1".to_vec()));
    assert_eq!(put(&r, &w, "k", b"v2", false), Ok(vec![1]));
    assert_eq!(get(&r, &w, "k"), Some(b"v2".to_vec()));
}

#[test]
fn if_absent_writes_once_and_reports_the_loss() {
    let r = rig();
    let w = who("geo", "t1");
    assert_eq!(put(&r, &w, "k", b"first", true), Ok(vec![1]));
    assert_eq!(put(&r, &w, "k", b"second", true), Ok(vec![0]));
    assert_eq!(get(&r, &w, "k"), Some(b"first".to_vec()));
}

#[test]
fn a_delete_says_whether_the_key_was_there() {
    let r = rig();
    let w = who("geo", "t1");
    let del = |r: &Rig| r.backend.call(&w, "state.delete", &encode_key("k"));
    assert_eq!(del(&r), Ok(vec![0]));
    put(&r, &w, "k", b"v", false).expect("put");
    assert_eq!(del(&r), Ok(vec![1]));
    assert_eq!(get(&r, &w, "k"), None);
}

#[test]
fn plugins_and_tenants_see_only_their_own_state() {
    let r = rig();
    put(&r, &who("geo", "t1"), "k", b"geo-t1", false).expect("put");
    put(&r, &who("geo", "t2"), "k", b"geo-t2", false).expect("put");
    put(&r, &who("ltr", "t1"), "k", b"ltr-t1", false).expect("put");
    put(&r, &who("geo", ""), "k", b"geo-node", false).expect("put");
    assert_eq!(get(&r, &who("geo", "t1"), "k"), Some(b"geo-t1".to_vec()));
    assert_eq!(get(&r, &who("geo", "t2"), "k"), Some(b"geo-t2".to_vec()));
    assert_eq!(get(&r, &who("ltr", "t1"), "k"), Some(b"ltr-t1".to_vec()));
    assert_eq!(get(&r, &who("geo", ""), "k"), Some(b"geo-node".to_vec()));
    assert_eq!(get(&r, &who("ltr", "t2"), "k"), None);
}

#[test]
fn a_bad_key_a_bad_input_and_an_unknown_op_are_refused() {
    let r = rig();
    let w = who("geo", "t1");
    for key in ["", "_hidden", "a/b", "a*"] {
        assert_eq!(
            put(&r, &w, key, b"v", false),
            Err(HostError::Invalid),
            "{key:?}"
        );
    }
    assert_eq!(
        r.backend.call(&w, "state.get", &[9, 9]),
        Err(HostError::Invalid)
    );
    assert_eq!(
        r.backend.call(&w, "state.list", &[]),
        Err(HostError::Unknown)
    );
    // Anything outside `state.` goes to the backend behind.
    assert_eq!(
        r.backend.call(&w, "search", &[]),
        NoBackend.call(&w, "search", &[])
    );
}

#[test]
fn a_node_with_no_room_for_a_call_is_unavailable() {
    let r = rig_with(1 << 10);
    assert_eq!(
        r.backend
            .call(&who("geo", "t1"), "state.get", &encode_key("k")),
        Err(HostError::Unavailable)
    );
}

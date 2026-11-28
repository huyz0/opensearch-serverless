use std::sync::Arc;

use oss_blob::{BlobStore, Emulator, S3Op};
use oss_format::DocKind;
use oss_meta::{DocSet, ScheduleRegistry};
use serde_json::{Value, json};

use super::backend::Unsupported;
use super::schedule::{NAMESPACE, Scheduler, TENANT};

const DAY: i64 = 86_400_000;
const BUF: usize = 1 << 16;

fn store() -> Arc<Emulator> {
    Arc::new(Emulator::new(1 << 30, 3))
}

fn dynstore(s: &Arc<Emulator>) -> Arc<dyn BlobStore> {
    Arc::clone(s) as Arc<dyn BlobStore>
}

fn requests(s: &Emulator) -> u64 {
    S3Op::ALL.iter().map(|op| s.meter().count(*op)).sum()
}

/// A policy `p` that moves `hot` to `ro` after a day, and one managed index.
fn attach(s: &Arc<Emulator>, index: &str, created: i64) {
    let policy = json!({"seq_no":1,"policy":{"policy_id":"p","default_state":"hot","states":[
        {"name":"hot","actions":[],"transitions":[{"state_name":"ro","conditions":{"min_index_age":"1d"}}]},
        {"name":"ro","actions":[{"read_only":{}}],"transitions":[]}]}});
    let mut marker = vec![0u8; BUF];
    DocSet::new(dynstore(s), "", DocKind::IsmPolicies)
        .unwrap()
        .put("p", policy.to_string().as_bytes(), false, &mut marker)
        .unwrap();
    let record = json!({"policy_id":"p","index":index,"index_uuid":"u","index_creation_date":created,
        "state":{"name":"hot","start_time":created},"enabled":true});
    DocSet::new(dynstore(s), "", DocKind::IsmManaged)
        .unwrap()
        .put(index, record.to_string().as_bytes(), false, &mut marker)
        .unwrap();
    ScheduleRegistry::in_namespace(dynstore(s), "", NAMESPACE)
        .unwrap()
        .register(TENANT)
        .unwrap();
}

fn record(s: &Arc<Emulator>, index: &str) -> Value {
    let mut marker = vec![0u8; BUF];
    let mut doc = vec![0u8; DocKind::IsmManaged.max_doc_bytes()];
    let n = DocSet::new(dynstore(s), "", DocKind::IsmManaged)
        .unwrap()
        .get(index, &mut marker, &mut doc)
        .unwrap()
        .expect("a record");
    serde_json::from_slice(&doc[..n]).unwrap()
}

fn node(s: &Arc<Emulator>, name: &str) -> Scheduler {
    Scheduler::new(&dynstore(s), "", Arc::new(Unsupported), name, 60_000, BUF).unwrap()
}

#[test]
fn a_tenant_with_no_managed_index_is_never_read_past_the_registry() {
    let s = store();
    let n = node(&s, "a");
    let r = n.tick(1_000, &["a"]).unwrap();
    assert!(r.refreshed && r.evaluated.is_empty());
}

#[test]
fn an_index_is_initialised_then_waits_then_transitions_at_its_derived_time() {
    let s = store();
    attach(&s, "logs", 1_000);
    let n = node(&s, "a");
    assert_eq!(n.tick(1_000, &["a"]).unwrap().evaluated, ["logs"]);
    assert_eq!(
        record(&s, "logs")["info"]["message"],
        "Successfully initialized policy: p"
    );
    assert_eq!(n.tick(1_001, &["a"]).unwrap().evaluated, ["logs"]);
    let waiting = record(&s, "logs");
    assert_eq!(waiting["step"]["step_status"], "condition_not_met");
    assert_eq!(waiting["next_eval"], 1_000 + DAY);

    let before = requests(&s);
    let r = n.tick(1_002, &["a"]).unwrap();
    assert!(!r.refreshed && r.evaluated.is_empty());
    assert_eq!(requests(&s), before, "nothing due costs no request");

    assert_eq!(n.tick(1_000 + DAY, &["a"]).unwrap().evaluated, ["logs"]);
    assert_eq!(record(&s, "logs")["state"]["name"], "ro");
}

#[test]
fn only_the_owner_evaluates_and_a_split_view_is_settled_by_the_claim() {
    let s = store();
    attach(&s, "logs", 1_000);
    let (a, b) = (node(&s, "a"), node(&s, "b"));
    let both = ["a", "b"];
    let ra = a.tick(1_000, &both).unwrap();
    let rb = b.tick(1_000, &both).unwrap();
    assert_eq!(ra.evaluated.len() + rb.evaluated.len(), 1, "one owner");

    // each believes it is alone: both own the index, one wins the slot
    let s = store();
    attach(&s, "logs", 1_000);
    let (a, b) = (node(&s, "a"), node(&s, "b"));
    b.tick(500, &["b"]).unwrap();
    let ra = a.tick(1_000, &["a"]).unwrap();
    let rb = b.tick(1_000, &["b"]).unwrap();
    assert_eq!(ra.evaluated, ["logs"]);
    assert_eq!(rb.lost, ["logs"]);
    assert!(rb.evaluated.is_empty());
}

/// A backend that deletes what it is asked to and remembers it.
#[derive(Default)]
struct Deleting(std::sync::Mutex<Vec<String>>);

impl super::backend::Backend for Deleting {
    fn set_read_only(&self, _index: &str, _on: bool) -> Result<(), String> {
        Ok(())
    }

    fn set_replicas(&self, _index: &str, _n: u32) -> Result<(), String> {
        Ok(())
    }

    fn delete(&self, index: &str) -> Result<(), String> {
        self.0.lock().unwrap().push(index.to_owned());
        Ok(())
    }
}

#[test]
fn a_deleted_index_leaves_the_managed_set() {
    let s = store();
    let policy = json!({"seq_no":1,"policy":{"policy_id":"p","default_state":"gone","states":[
        {"name":"gone","actions":[{"delete":{}}],"transitions":[]}]}});
    let mut marker = vec![0u8; BUF];
    DocSet::new(dynstore(&s), "", DocKind::IsmPolicies)
        .unwrap()
        .put("p", policy.to_string().as_bytes(), false, &mut marker)
        .unwrap();
    let record = json!({"policy_id":"p","index":"logs","index_uuid":"u","index_creation_date":1,
        "state":{"name":"gone","start_time":1},"enabled":true,
        "info":{"message":"Successfully initialized policy: p"},"next_eval":1000});
    DocSet::new(dynstore(&s), "", DocKind::IsmManaged)
        .unwrap()
        .put("logs", record.to_string().as_bytes(), false, &mut marker)
        .unwrap();
    ScheduleRegistry::in_namespace(dynstore(&s), "", NAMESPACE)
        .unwrap()
        .register(TENANT)
        .unwrap();
    let backend = Arc::new(Deleting::default());
    let n = Scheduler::new(
        &dynstore(&s),
        "",
        Arc::clone(&backend) as _,
        "a",
        60_000,
        BUF,
    )
    .unwrap();
    assert_eq!(n.tick(1_000, &["a"]).unwrap().evaluated, ["logs"]);
    assert_eq!(*backend.0.lock().unwrap(), ["logs"]);
    let names = DocSet::new(dynstore(&s), "", DocKind::IsmManaged)
        .unwrap()
        .names(&mut marker)
        .unwrap();
    assert!(names.is_empty(), "{names:?}");
    n.tick(1_001, &["a"]).unwrap();
    assert_eq!(backend.0.lock().unwrap().len(), 1, "deleted once");
}

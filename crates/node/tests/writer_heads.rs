//! Admission over the shard head and the incarnation fence
//! (docs/specs/shard-head/SPEC.md, AC-8).

use std::sync::Arc;
use std::time::Duration;

use oss_blob::{BlobStore, Emulator};
use oss_cluster::heads::ShardHeadStore;
use oss_cluster::{LeaseConfig, LivenessOracle, Membership, SystemClock};
use oss_format::ShardKey;
use oss_node::writer::{HeadAdmission, Incarnation, ShardNames};
use oss_writer::log::ShardAdmission;

fn rig() -> (Arc<ShardHeadStore>, Arc<ShardNames>) {
    let store: Arc<dyn BlobStore> = Arc::new(Emulator::new(1 << 24, 1));
    let clock = Arc::new(SystemClock);
    let membership = Arc::new(
        Membership::new(
            Arc::clone(&store),
            "",
            clock.clone(),
            LeaseConfig {
                ttl_millis: 30_000,
                max_members: 8,
                conditional_deletes: false,
            },
        )
        .expect("membership"),
    );
    let heads = ShardHeadStore::new(
        store,
        "",
        clock,
        30_000,
        Arc::new(LivenessOracle::new(membership)),
    )
    .expect("heads");
    let names = Arc::new(ShardNames::default());
    names.insert("u1", "logs");
    (Arc::new(heads), names)
}

fn key() -> ShardKey {
    ShardKey::new("u1", 0, None::<&str>)
}

#[test]
fn admission_passes_the_owner_at_its_term_and_refuses_everything_else() {
    let (heads, names) = rig();
    heads
        .acquire("logs", 0, "n1", "e1", Some("u1"))
        .expect("take");
    let admit = |node: &str, eph: &str, term: i64| {
        HeadAdmission::new(Arc::clone(&heads), Arc::clone(&names), node, eph).admit(&key(), term, 1)
    };
    assert!(admit("n1", "e1", 1).is_ok());
    assert!(admit("n2", "e1", 1).is_err(), "another node");
    assert!(admit("n1", "e2", 1).is_err(), "another process");
    assert!(admit("n1", "e1", 2).is_err(), "another term");
    let unknown = ShardKey::new("nope", 0, None::<&str>);
    let by_name = HeadAdmission::new(Arc::clone(&heads), Arc::clone(&names), "n1", "e1");
    assert!(by_name.admit(&unknown, 1, 1).is_err(), "an index not held");
    let no_head = ShardKey::new("u1", 5, None::<&str>);
    assert!(by_name.admit(&no_head, 1, 1).is_err(), "no head");
}

struct Counting(std::sync::atomic::AtomicU32, bool);

impl Incarnation for Counting {
    fn live(&self, _: &str, _: &str) -> Result<bool, String> {
        self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(self.1)
    }
}

#[test]
fn a_writer_reads_the_incarnation_once_a_window_and_refuses_a_replaced_index() {
    use oss_node::writer::{RoleTarget, Routes, WriterRole};
    use oss_writer::bulk::{
        Bulk, BulkBudget, Defaults, IdGenerator, IndexInfo, ItemResult, Owner, Planned,
        StreamConfig, stream,
    };
    use oss_writer::log::{CommitterConfig, EpochLease, NodeLogCommitter, NodeLogStore, Refusal};
    use oss_writer::mapping::Mapping;

    struct Open;
    impl ShardAdmission for Open {
        fn admit(&self, _: &ShardKey, _: i64, _: u64) -> Result<(), Refusal> {
            Ok(())
        }
    }
    struct NoLease;
    impl EpochLease for NoLease {
        fn record(&self, _: &[oss_format::NodeLogEpoch]) -> Result<(), Refusal> {
            Ok(())
        }
    }
    struct Local;
    impl Routes for Local {
        fn index(&self, name: &str) -> Option<IndexInfo> {
            Some(info(name))
        }
        fn owner(&self, _: &IndexInfo, _: u32) -> Owner {
            Owner::Local
        }
        fn forward(&self, _: &str, _: &IndexInfo, _: u32, _: &[Planned<'_>]) -> Vec<ItemResult> {
            unreachable!()
        }
    }
    fn info(name: &str) -> IndexInfo {
        IndexInfo {
            name: name.into(),
            uuid: "u1".into(),
            shards: 1,
            partition_size: 1,
            routing_shards: 1,
            routing_required: false,
        }
    }
    fn write(role: &WriterRole, id: &str) -> bool {
        let defaults = Defaults {
            index: Some("logs".into()),
            ..Defaults::default()
        };
        let budget = BulkBudget::new(1 << 20, 2);
        let config = StreamConfig {
            slice_bytes: 1 << 20,
            max_wait: Duration::from_secs(5),
        };
        let mut target = RoleTarget::new(role);
        let mut ids = IdGenerator::new(0, [0; 6]);
        let mut buffer = [0u8; 1024];
        let mut run = Bulk::new(&mut target, &mut ids, 1, false);
        let body = format!("{{\"index\":{{\"_id\":\"{id}\"}}}}\n{{\"n\":1}}\n");
        stream(
            &mut body.as_bytes(),
            &mut buffer,
            &defaults,
            &budget,
            config,
            &mut run,
        )
        .expect("streams");
        matches!(
            run.finish().expect("ends")[0].result,
            ItemResult::Written { .. }
        )
    }

    for (live, writes) in [(true, true), (false, false)] {
        let store: Arc<dyn BlobStore> = Arc::new(Emulator::new(1 << 24, 1));
        let log = NodeLogStore::new(store, "", "n1").expect("names");
        let committer = Arc::new(NodeLogCommitter::new(
            log,
            1,
            Box::new(NoLease),
            Box::new(Open),
            CommitterConfig {
                window: Duration::ZERO,
                ..CommitterConfig::default()
            },
        ));
        let check = Arc::new(Counting(std::sync::atomic::AtomicU32::new(0), live));
        let role = WriterRole::new(committer, Arc::new(Local))
            .with_incarnation(check.clone(), Duration::from_secs(600));
        role.open_shard(&info("logs"), 0, 1, Mapping::default())
            .expect("opens");
        assert_eq!(write(&role, "a"), writes);
        assert_eq!(write(&role, "b"), writes);
        let reads = check.0.load(std::sync::atomic::Ordering::SeqCst);
        if live {
            assert_eq!(reads, 1, "once inside the window");
        } else {
            assert!(!role.held("u1", 0), "a replaced index's shard is released");
        }
    }
}

//! docs/specs/node-bucket-wiring: a member shard publishes into its bucket.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use oss_blob::{BlobStore, Emulator, S3Op};
use oss_cluster::{LeaseClock, SystemClock};
use oss_format::{
    BucketManifest, BucketMember, CommitManifest, IndexRecord, NodeLogEpoch, ShardKey, ShardRef,
};
use oss_meta::{DeleteMode, DescriptorStore};
use oss_node::writer::{BucketWiring, Publishing, RoleTarget, Routes, WriterRole};
use oss_writer::bucket::{BucketCommits, BucketStore, Limits, ManifestWrite};
use oss_writer::bulk::{
    Bulk, BulkBudget, Defaults, IdGenerator, IndexInfo, ItemResult, Owner, Planned, StreamConfig,
    stream,
};
use oss_writer::log::{
    CommitterConfig, EpochLease, NodeLogCommitter, NodeLogStore, Refusal, ShardAdmission,
};
use oss_writer::mapping::Mapping;
use oss_writer::publish::lazy::Policy;

const BUCKET: &str = "s02-1";

struct Lease;

impl EpochLease for Lease {
    fn record(&self, _: &[NodeLogEpoch]) -> Result<(), Refusal> {
        Ok(())
    }
}

struct Admit;

impl ShardAdmission for Admit {
    fn admit(&self, _: &ShardKey, _: i64, _: u64) -> Result<(), Refusal> {
        Ok(())
    }
}

struct Local(IndexInfo);

impl Routes for Local {
    fn index(&self, _: &str) -> Option<IndexInfo> {
        Some(self.0.clone())
    }

    fn owner(&self, _: &IndexInfo, _: u32) -> Owner {
        Owner::Local
    }

    fn forward(&self, _: &str, _: &IndexInfo, _: u32, _: &[Planned<'_>]) -> Vec<ItemResult> {
        Vec::new()
    }
}

fn info(name: &str) -> IndexInfo {
    IndexInfo {
        name: name.into(),
        uuid: format!("{name}-uuid"),
        shards: 1,
        partition_size: 1,
        routing_shards: 1,
        routing_required: false,
    }
}

struct Rig {
    store: Arc<Emulator>,
    role: WriterRole,
    wiring: Arc<BucketWiring>,
    commits: BucketCommits,
}

/// A node whose descriptors have `logs` in a bucket and `solo` in none; the
/// bucket's manifest lists `logs` as a member.
fn rig(index: &str) -> Rig {
    let store = Arc::new(Emulator::new(1 << 30, 7));
    let dynstore = || Arc::clone(&store) as Arc<dyn BlobStore>;
    let descriptors =
        Arc::new(DescriptorStore::new(dynstore(), "", DeleteMode::Tombstone).expect("descriptors"));
    let mut buf = vec![0u8; 64 << 10];
    let mut logs = IndexRecord::new("logs", "logs-uuid", 1);
    logs.bucket = Some(BUCKET.into());
    descriptors.create(&logs, 1, &mut buf).expect("logs");
    descriptors
        .create(&IndexRecord::new("solo", "solo-uuid", 1), 1, &mut buf)
        .expect("solo");
    let write = ManifestWrite::Json;
    let manifest = BucketManifest::empty(BUCKET)
        .unwrap()
        .at_term(1)
        .unwrap()
        .with_member("logs-uuid", BucketMember::first("logs", None).unwrap())
        .unwrap();
    assert!(
        BucketStore::new(dynstore(), "", write)
            .create(&manifest)
            .unwrap()
    );
    let clock: Arc<dyn LeaseClock> = Arc::new(SystemClock);
    let wiring = Arc::new(BucketWiring::new(
        dynstore(),
        "",
        Arc::clone(&descriptors),
        clock,
        write,
        Limits::new(1 << 20, 8, 1 << 16),
    ));
    let log = NodeLogStore::new(dynstore(), "", "node-1").expect("names");
    let committer = Arc::new(NodeLogCommitter::new(
        log,
        7,
        Box::new(Lease),
        Box::new(Admit),
        CommitterConfig {
            window: Duration::from_millis(0),
            ..CommitterConfig::default()
        },
    ));
    let clock = Arc::new(AtomicU64::new(0));
    let publishing = Publishing::new(
        dynstore(),
        "",
        "node-1",
        Policy::new(0, 0, 250),
        Box::new(move || clock.load(Ordering::SeqCst)),
    )
    .with_buckets(Arc::clone(&wiring));
    let role = WriterRole::new(committer, Arc::new(Local(info(index)))).with_publishing(publishing);
    role.open_shard(
        &info(index),
        0,
        3,
        Mapping::parse(br#"{"properties":{"n":{"type":"integer"}}}"#).expect("mapping"),
    )
    .expect("opens");
    let commits = BucketCommits::new(BucketStore::new(dynstore(), "", write));
    Rig {
        store,
        role,
        wiring,
        commits,
    }
}

fn write(role: &WriterRole, index: &str) {
    let defaults = Defaults {
        index: Some(index.into()),
        ..Defaults::default()
    };
    let budget = BulkBudget::new(1 << 20, 2);
    let config = StreamConfig {
        slice_bytes: 1 << 20,
        max_wait: Duration::from_secs(5),
    };
    let mut target = RoleTarget::new(role);
    let mut ids = IdGenerator::new(0, [0; 6]);
    let mut buffer = [0u8; 4096];
    let mut run = Bulk::new(&mut target, &mut ids, 1, false);
    let mut body: &[u8] = b"{\"index\":{}}\n{\"n\":1}\n";
    stream(&mut body, &mut buffer, &defaults, &budget, config, &mut run).expect("streams");
    run.finish().expect("finishes");
}

#[test]
fn a_bucketed_index_has_a_member_register_and_others_none() {
    let r = rig("logs");
    assert!(r.wiring.register("logs", 0, "logs-uuid").unwrap().is_some());
    assert!(r.wiring.register("solo", 0, "solo-uuid").unwrap().is_none());
    assert!(r.wiring.register("ghost", 0, "g").unwrap().is_none());
    // another shard than 0 of a bucketed index is not a member
    assert!(r.wiring.register("logs", 1, "logs-uuid").unwrap().is_none());
}

#[test]
fn a_member_shards_publish_lands_in_its_bucket_entry() {
    let r = rig("logs");
    write(&r.role, "logs");
    let puts = r.store.meter().count(S3Op::Put);
    assert_eq!(r.role.publish_due().published, 1);
    assert_eq!(
        r.store.meter().count(S3Op::Put) - puts,
        2,
        "a blob and a bucket swap"
    );
    let newest = r.role.newest_commit("logs-uuid", 0).expect("staged");
    let entry = r
        .commits
        .member_commit(BUCKET, "logs-uuid")
        .unwrap()
        .unwrap();
    assert_eq!(entry.version, 1);
    assert_eq!(entry.commit.as_slice(), &newest[..]);
    assert_eq!(CommitManifest::parse(&entry.commit).unwrap().term, 3);
    let own = ShardRef {
        index: "logs",
        uuid: "logs-uuid",
        shard: 0,
    }
    .manifest_key("")
    .unwrap();
    assert!(
        r.store
            .read_register(&own, &mut vec![0; 4096])
            .unwrap()
            .is_none(),
        "the shard's own register is not written"
    );
}

#[test]
fn a_shard_of_an_unbucketed_index_publishes_to_its_own_register() {
    let r = rig("solo");
    write(&r.role, "solo");
    assert_eq!(r.role.publish_due().published, 1);
    let own = ShardRef {
        index: "solo",
        uuid: "solo-uuid",
        shard: 0,
    }
    .manifest_key("")
    .unwrap();
    assert!(
        r.store
            .read_register(&own, &mut vec![0; 1 << 16])
            .unwrap()
            .is_some()
    );
    assert!(
        r.commits
            .member_commit(BUCKET, "logs-uuid")
            .unwrap()
            .is_none(),
        "the bucket's member has no commit"
    );
}

#[test]
fn a_registered_node_reports_the_members_it_holds_open() {
    let r = rig("logs");
    let names = Arc::new(oss_node::writer::ShardNames::default());
    let role = Arc::new(r.role.with_names(Arc::clone(&names)));
    names.insert("logs-uuid", "logs");
    let heads = r.wiring.heads();
    // Nothing is registered yet: no sibling is known open.
    assert!(!heads.others_open_on(BUCKET, "node-1", "other").unwrap());
    r.wiring.register_open("node-1", Arc::clone(&role));
    assert!(heads.others_open_on(BUCKET, "node-1", "other").unwrap());
    assert!(!heads.others_open_on(BUCKET, "node-1", "logs").unwrap());
    assert!(!heads.others_open_on(BUCKET, "node-2", "other").unwrap());
}

#[test]
fn a_join_places_the_index_in_a_bucket_listing_it_as_a_member() {
    let r = rig("logs");
    let bucket = r
        .wiring
        .join("fresh", "fresh", "fresh-uuid")
        .expect("joins");
    assert!(!bucket.is_empty());
    let read = BucketStore::new(
        Arc::clone(&r.store) as Arc<dyn BlobStore>,
        "",
        ManifestWrite::Json,
    )
    .read(&bucket)
    .expect("reads")
    .expect("exists");
    assert!(read.manifest.member("fresh-uuid").is_some());
    // The same index again lands in the same bucket.
    assert_eq!(
        r.wiring
            .join("fresh", "fresh", "fresh-uuid")
            .expect("joins"),
        bucket
    );
}

fn reader_store(r: &Rig) -> oss_blob::PooledStore<oss_blob::Shared> {
    use oss_mem::{BudgetConfig, BufferPool, MemoryBudget, Pool};
    let config = BudgetConfig::new(8 << 20)
        .with_pool(Pool::Transport, 8 << 20)
        .expect("fits");
    let buffers =
        BufferPool::new(&MemoryBudget::new(&config), Pool::Transport, 1 << 16, 16).expect("fits");
    oss_blob::PooledStore::new(
        oss_blob::Shared::new(Arc::clone(&r.store) as Arc<dyn BlobStore>),
        buffers,
    )
}

#[test]
fn a_reader_opens_a_member_from_its_bucket_entry() {
    use oss_reader::shard_files::ShardFiles;
    let r = rig("logs");
    write(&r.role, "logs");
    assert_eq!(r.role.publish_due().published, 1);
    let store = reader_store(&r);
    let gets = r.store.meter().count(S3Op::Get);
    let (files, meta) = ShardFiles::open_with_meta(&store, "", "logs", 0).expect("opens");
    assert_eq!(
        r.store.meter().count(S3Op::Get) - gets,
        2,
        "descriptor and bucket manifest"
    );
    assert_eq!(files.term, 3);
    assert!(!files.is_empty());
    assert_eq!(meta.uuid, "logs-uuid");
    // A bucketed index has only shard 0.
    assert!(ShardFiles::open_with_meta(&store, "", "logs", 1).is_err());
}

#[test]
fn a_member_with_no_commit_yet_is_not_followable() {
    use oss_reader::shard_files::ShardFiles;
    let r = rig("logs");
    let store = reader_store(&r);
    let err = ShardFiles::open_with_meta(&store, "", "logs", 0).expect_err("no commit");
    assert!(err.to_string().contains("no commit"), "{err}");
}

#[test]
fn leaving_takes_the_index_back_out_of_its_bucket() {
    let r = rig("logs");
    let bucket = r
        .wiring
        .join("fresh", "fresh", "fresh-uuid")
        .expect("joins");
    let manifest = |r: &Rig| {
        BucketStore::new(
            Arc::clone(&r.store) as Arc<dyn BlobStore>,
            "",
            ManifestWrite::Json,
        )
        .read(&bucket)
        .expect("reads")
        .expect("exists")
        .manifest
    };
    assert!(manifest(&r).member("fresh-uuid").is_some());
    r.wiring.leave(&bucket, "fresh-uuid").expect("leaves");
    assert!(manifest(&r).member("fresh-uuid").is_none());
}

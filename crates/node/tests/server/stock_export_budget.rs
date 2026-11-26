//! docs/specs/stock-snapshot-export AC-10: what taking the recorded 3.10
//! shard into an `s3` stock repository costs, the node's store and the
//! target together.

use std::sync::Arc;

use oss_blob::{BlobStore, ByteRange, Emulator, S3Op, SliceSource, SourceRoots};
use oss_format::ShardRef;
use oss_node::stock_export_route::StockExportSettings;
use oss_testkit::budget;

use crate::stock_export::{Rig, TAKE, WAIT, rig_with, send};

const REPO: &str =
    r#"{"type":"s3","settings":{"format":"stock","bucket":"exportdst","base_path":"out"}}"#;

fn all(store: &Emulator) -> u64 {
    S3Op::ALL.iter().map(|op| store.meter().count(*op)).sum()
}

fn target() -> (Arc<Emulator>, oss_blob::emulator::http::S3Server) {
    let emulator = Arc::new(Emulator::new(64 << 20, 3));
    let server = oss_blob::emulator::http::S3Server::start(
        Arc::clone(&emulator),
        "exportdst",
        "us-east-1",
        oss_blob::Credentials::new("dstaccess", "dstsecret"),
    )
    .expect("the S3 front end");
    (emulator, server)
}

fn rig(server: &oss_blob::emulator::http::S3Server) -> Rig {
    rig_at(server.endpoint(), "dstaccess", "dstsecret")
}

fn rig_at(endpoint: String, access: &str, secret: &str) -> Rig {
    let settings = StockExportSettings {
        enabled: true,
        roots: SourceRoots::parse("", "exportdst/out"),
        s3: Some((
            oss_node::stock_s3::SourceS3 {
                endpoint,
                region: "us-east-1".to_owned(),
                credentials: oss_blob::Credentials::new(access, secret),
                own_bucket: "own".to_owned(),
            },
            String::new(),
        )),
        chunk_bytes: 1 << 20,
    };
    let root = std::env::temp_dir().join(format!("oss-export-budget-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    rig_with(settings, root, REPO)
}

#[test]
fn a_stock_export_costs_what_the_budget_says() {
    let (dst, server) = target();
    let r = rig(&server);
    let (own, to) = (all(&r.store), all(&dst));
    let reply = send(&r, "PUT", &format!("/_snapshot/stock/s1{WAIT}"), Some(TAKE));
    assert_eq!(reply.status, 200, "{}", reply.text());
    assert_eq!(dst.meter().count(S3Op::List), 0);
    assert_eq!(r.store.meter().count(S3Op::List), 0);
    let detail = format!("node {} target {}", all(&r.store) - own, all(&dst) - to);
    budget::check(
        "snapshot.stock_export.requests",
        (all(&r.store) - own) + (all(&dst) - to),
        &detail,
    );
}

#[test]
fn a_failed_stock_export_costs_what_the_budget_says() {
    let (dst, server) = target();
    let r = rig(&server);
    // One file's byte changed: its length is right and its checksum is not.
    let at = ShardRef {
        index: "logs",
        uuid: "u1",
        shard: 0,
    };
    let key = format!("{}t=1/_0.cfs", at.base_path("").unwrap());
    let mut bytes = Vec::new();
    r.store
        .get(&key, ByteRange::Full, &mut [0u8; 64], &mut |c: &[u8]| {
            bytes.extend_from_slice(c);
            Ok(())
        })
        .unwrap();
    bytes[3] ^= 1;
    r.store
        .put(&key, &mut SliceSource::new(&bytes), &mut [0u8; 64])
        .unwrap();
    let (own, to) = (all(&r.store), all(&dst));
    let reply = send(&r, "PUT", &format!("/_snapshot/stock/s1{WAIT}"), Some(TAKE));
    assert!(reply.status >= 400, "{}", reply.text());
    let detail = format!("node {} target {}", all(&r.store) - own, all(&dst) - to);
    budget::check(
        "snapshot.stock_export_cleanup.requests",
        (all(&r.store) - own) + (all(&dst) - to),
        &detail,
    );
}

/// The same take into a RustFS target, when `RUSTFS_BIN` names one: it
/// succeeds, the node's own store makes no LIST, and the target holds as many
/// objects as the emulator target does. RustFS has no meter, so the target's
/// own request count is not read here.
#[test]
fn a_stock_export_to_rustfs_writes_what_the_emulator_target_holds() {
    let Some((_rustfs, endpoint)) = crate::rustfs_rig::start("exportdst") else {
        eprintln!("skipped: RUSTFS_BIN is not set");
        return;
    };
    let (dst, server) = target();
    let reference = rig(&server);
    let reply = send(
        &reference,
        "PUT",
        &format!("/_snapshot/stock/s1{WAIT}"),
        Some(TAKE),
    );
    assert_eq!(reply.status, 200, "{}", reply.text());

    let r = rig_at(
        endpoint.clone(),
        crate::rustfs_rig::RUSTFS_ACCESS,
        crate::rustfs_rig::RUSTFS_SECRET,
    );
    let reply = send(&r, "PUT", &format!("/_snapshot/stock/s1{WAIT}"), Some(TAKE));
    assert_eq!(reply.status, 200, "{}", reply.text());
    assert_eq!(r.store.meter().count(S3Op::List), 0);

    let held = oss_blob::S3Store::new(oss_blob::S3Config::new(
        &endpoint,
        "exportdst",
        "us-east-1",
        oss_blob::Credentials::new(
            crate::rustfs_rig::RUSTFS_ACCESS,
            crate::rustfs_rig::RUSTFS_SECRET,
        ),
    ))
    .unwrap();
    let mut keys = Vec::new();
    // LIST: one page of the test's own check of what the target holds.
    held.list_prefix("out/", None, 1000, &mut |k, _| keys.push(k.to_owned()))
        .unwrap();
    assert!(keys.iter().any(|k| k == "out/index.latest"), "{keys:?}");
    assert_eq!(keys.len(), dst.object_count(), "{keys:?}");
}

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
    let settings = StockExportSettings {
        enabled: true,
        roots: SourceRoots::parse("", "exportdst/out"),
        s3: Some((
            oss_node::stock_s3::SourceS3 {
                endpoint: server.endpoint(),
                region: "us-east-1".to_owned(),
                credentials: oss_blob::Credentials::new("dstaccess", "dstsecret"),
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

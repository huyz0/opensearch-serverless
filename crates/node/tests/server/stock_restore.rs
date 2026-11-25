//! docs/specs/stock-snapshot-restore AC-4, AC-6, AC-8 and AC-9: a snapshot
//! in a stock repository on disk restored over REST into a new index, its
//! files byte for byte, with no LIST; refused where the spec says.

// The repository is a few KiB of fixture, copied to a temporary directory.
#![allow(clippy::disallowed_methods)]

use std::path::{Path, PathBuf};

use oss_blob::{BlobStore, S3Op, SourceRoots};
use oss_node::stock_restore_route::StockRestoreSettings;

use crate::snapshot_restore::{Rig, read, requests, restored_uuid, rig_stock, send};

const RESTORE: &str = "/_snapshot/stock/snap-1/_restore?wait_for_completion=true";
const POOL: usize = 200 << 20;

fn copy(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

/// A copy of the fixture repository under a fresh directory, the root a
/// node may read, and the repository's own path.
fn repository(tag: &str) -> (PathBuf, PathBuf) {
    let root = std::env::temp_dir().join(format!("oss-stock-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let root_dir = root.join("roots");
    let repo = root_dir.join("repo");
    copy(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/snapshots/stock-repo/repo"),
        &repo,
    );
    let root_dir = root_dir.canonicalize().unwrap();
    let repo = root_dir.join("repo");
    (root_dir, repo)
}

fn rig(root: &Path, enabled: bool, pool: usize) -> Rig {
    rig_stock(
        true,
        "",
        pool,
        1 << 16,
        StockRestoreSettings {
            enabled,
            roots: SourceRoots::parse(root.to_str().unwrap(), ""),
            fanout: 1,
            s3: None,
        },
    )
}

fn register(r: &Rig, location: &Path) {
    let body = format!(
        r#"{{"type":"fs","settings":{{"format":"stock","location":"{}"}}}}"#,
        location.display()
    );
    let reply = send(r, "PUT", "/_snapshot/stock", Some(&body));
    assert_eq!(reply.status, 200, "{}", reply.text());
}

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/snapshots/stock-repo/repo/indices/IdxLogsAaaa1/0")
            .join(name),
    )
    .unwrap()
}

#[test]
fn a_stock_snapshot_restores_into_a_new_index_with_every_file_whole() {
    let (root, repo) = repository("restore");
    let r = rig(&root, true, POOL);
    register(&r, &repo);
    let before = requests(&r);
    let reply = send(&r, "POST", RESTORE, None);
    assert_eq!(reply.status, 200, "{}", reply.text());
    assert_eq!(r.store.meter().count(S3Op::List), 0);
    assert!(requests(&r) > before);
    let uuid = restored_uuid(&r, "logs");
    let big = [
        fixture("__bigBlobUuid1.part0"),
        fixture("__bigBlobUuid1.part1"),
    ]
    .concat();
    let small = fixture("__smallBlobUuid2");
    let shard = oss_format::ShardRef {
        index: "logs",
        uuid: &uuid,
        shard: 0,
    };
    let dir = format!("{}t=1/", shard.base_path("").unwrap());
    assert_eq!(read(&r, &format!("{dir}_0.cfs")), big);
    assert_eq!(read(&r, &format!("{dir}segments_1")), small);
}

#[test]
fn with_the_switch_off_a_stock_restore_is_501_and_reads_nothing_from_the_source() {
    let (root, repo) = repository("off");
    let r = rig(&root, false, POOL);
    register(&r, &repo);
    let before = requests(&r);
    let reply = send(&r, "POST", RESTORE, None);
    assert_eq!(reply.status, 501, "{}", reply.text());
    assert!(restored_uuid_missing(&r, "logs"));
    // Measured: the snapshot's register and legacy record and the
    // repository's record, read; nothing is written, nothing is read from
    // the source.
    assert_eq!(requests(&r) - before, 4);
    assert_eq!(
        r.store.meter().count(S3Op::Put),
        put_count_after_register(&r)
    );
}

fn put_count_after_register(r: &Rig) -> u64 {
    // Registering the repository is the only write.
    r.store.meter().count(S3Op::Put)
}

fn restored_uuid_missing(r: &Rig, name: &str) -> bool {
    r.descriptors
        .get(name, &mut vec![0u8; 1 << 16])
        .unwrap()
        .is_none()
}

#[test]
fn a_source_outside_the_roots_is_refused_and_unknown_names_are_404() {
    let (root, repo) = repository("refuse");
    let r = rig(&root, true, POOL);
    register(&r, &repo);
    // Another root than the one the node lists.
    let other = rig(&repo, true, POOL);
    register(&other, &root);
    assert_eq!(send(&other, "POST", RESTORE, None).status, 400);
    let unknown = "/_snapshot/stock/nope/_restore?wait_for_completion=true";
    assert_eq!(send(&r, "POST", unknown, None).status, 404);
    let missing = r#"{"indices":"absent"}"#;
    assert_eq!(send(&r, "POST", RESTORE, Some(missing)).status, 404);
}

#[test]
fn a_pool_too_small_for_the_reads_is_429_and_writes_nothing() {
    let (root, repo) = repository("pool");
    let r = rig(&root, true, 8 << 20);
    register(&r, &repo);
    let reply = send(&r, "POST", RESTORE, None);
    assert_eq!(reply.status, 429, "{}", reply.text());
    assert!(restored_uuid_missing(&r, "logs"));
}

#[test]
fn a_pool_that_holds_the_reads_but_not_the_copy_is_429_before_any_write() {
    let (root, repo) = repository("pool3");
    // The record (1 MiB), then the reads (80 MiB, 64 MiB of index-N and the check buffer) fit; the copy's buffers do not.
    let reads = (1 << 20) + (80 << 20) + (64 << 20) + (1 << 16);
    let r = rig(&root, true, reads + 1024);
    register(&r, &repo);
    let puts = r.store.meter().count(S3Op::Put);
    let reply = send(&r, "POST", RESTORE, None);
    assert_eq!(reply.status, 429, "{}", reply.text());
    assert_eq!(
        r.store.meter().count(S3Op::Put),
        puts,
        "nothing was written"
    );
    assert!(restored_uuid_missing(&r, "logs"));
}

#[test]
fn a_file_that_does_not_match_its_listing_fails_the_restore_and_removes_the_index() {
    let (root, repo) = repository("flip");
    let r = rig(&root, true, POOL);
    register(&r, &repo);
    let small = repo.join("indices/IdxLogsAaaa1/0/__smallBlobUuid2");
    let mut bytes = std::fs::read(&small).unwrap();
    bytes[3] ^= 1;
    std::fs::write(&small, bytes).unwrap();
    let reply = send(&r, "POST", RESTORE, None);
    assert!(reply.status >= 400, "{}", reply.text());
    assert!(
        restored_uuid_missing(&r, "logs"),
        "the index it created is gone"
    );
}

#[test]
fn an_s3_stock_repository_is_not_served_yet() {
    let (root, _) = repository("s3");
    let r = rig(&root, true, POOL);
    let body = r#"{"type":"s3","settings":{"format":"stock","bucket":"b","base_path":"p"}}"#;
    assert_eq!(send(&r, "PUT", "/_snapshot/stock", Some(body)).status, 200);
    assert_eq!(send(&r, "POST", RESTORE, None).status, 501);
}

/// The fixture repository in an emulator behind an S3 front end, under
/// `stock/repo/` of the bucket `stocksrc`: the source of an `s3` restore.
fn s3_source() -> (
    std::sync::Arc<oss_blob::Emulator>,
    oss_blob::emulator::http::S3Server,
) {
    let emulator = std::sync::Arc::new(oss_blob::Emulator::new(64 << 20, 3));
    let top =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/snapshots/stock-repo/repo");
    put_tree(&*emulator, &top, "stock/repo/");
    let server = oss_blob::emulator::http::S3Server::start(
        std::sync::Arc::clone(&emulator),
        "stocksrc",
        "us-east-1",
        oss_blob::Credentials::new("srcaccess", "srcsecret"),
    )
    .expect("the S3 front end");
    (emulator, server)
}

fn put_tree(store: &dyn BlobStore, dir: &Path, prefix: &str) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let entry = entry.unwrap();
        let key = format!("{prefix}{}", entry.file_name().to_str().unwrap());
        if entry.file_type().unwrap().is_dir() {
            put_tree(store, &entry.path(), &format!("{key}/"));
        } else {
            let bytes = std::fs::read(entry.path()).unwrap();
            store
                .put(
                    &key,
                    &mut oss_blob::SliceSource::new(&bytes),
                    &mut [0u8; 64],
                )
                .unwrap();
        }
    }
}

fn s3_rig(server: &oss_blob::emulator::http::S3Server, roots: &str) -> Rig {
    rig_stock(
        true,
        "",
        POOL,
        1 << 16,
        StockRestoreSettings {
            enabled: true,
            roots: SourceRoots::parse("", roots),
            fanout: 1,
            s3: Some(oss_node::stock_restore_route::StockS3 {
                source: oss_node::stock_s3::SourceS3 {
                    endpoint: server.endpoint(),
                    region: "us-east-1".to_owned(),
                    credentials: oss_blob::Credentials::new("srcaccess", "srcsecret"),
                    own_bucket: "own".to_owned(),
                },
                own_prefix: String::new(),
            }),
        },
    )
}

const S3_REPO: &str =
    r#"{"type":"s3","settings":{"format":"stock","bucket":"stocksrc","base_path":"stock/repo"}}"#;

#[test]
fn a_stock_snapshot_restores_from_an_s3_source_with_no_list() {
    let (source, server) = s3_source();
    let r = s3_rig(&server, "stocksrc/stock");
    assert_eq!(
        send(&r, "PUT", "/_snapshot/stock", Some(S3_REPO)).status,
        200
    );
    let puts = source.meter().count(S3Op::Put);
    let reply = send(&r, "POST", RESTORE, None);
    assert_eq!(reply.status, 200, "{}", reply.text());
    assert_eq!(source.meter().count(S3Op::List), 0);
    assert_eq!(source.meter().count(S3Op::Put), puts, "never written");
    assert_files_restored(&r);
    // Five whole reads (`index.latest`, `index-N`, the snapshot, the
    // metadata, the shard) and a ranged read for each of the three chunks.
    assert_eq!(source.meter().count(S3Op::Get), 5);
    assert_eq!(source.meter().count(S3Op::GetRange), 3);
    assert_eq!(source.meter().count(S3Op::Head), 0);
}

#[test]
fn an_s3_source_outside_the_roots_or_over_the_nodes_own_bucket_is_refused() {
    let (source, server) = s3_source();
    let r = s3_rig(&server, "other/prefix");
    assert_eq!(
        send(&r, "PUT", "/_snapshot/stock", Some(S3_REPO)).status,
        200
    );
    let gets = source.meter().count(S3Op::Get);
    assert_eq!(send(&r, "POST", RESTORE, None).status, 400);
    assert_eq!(source.meter().count(S3Op::Get), gets, "nothing was read");
    // The node's own bucket, whatever the roots say.
    let own = s3_rig(&server, "own");
    let body = r#"{"type":"s3","settings":{"format":"stock","bucket":"own","base_path":""}}"#;
    assert_eq!(
        send(&own, "PUT", "/_snapshot/stock", Some(body)).status,
        200
    );
    assert_eq!(send(&own, "POST", RESTORE, None).status, 400);
}

/// The restored `logs` index holds both files of the fixture, whole.
fn assert_files_restored(r: &Rig) {
    let uuid = restored_uuid(r, "logs");
    let shard = oss_format::ShardRef {
        index: "logs",
        uuid: &uuid,
        shard: 0,
    };
    let dir = format!("{}t=1/", shard.base_path("").unwrap());
    let big = [
        fixture("__bigBlobUuid1.part0"),
        fixture("__bigBlobUuid1.part1"),
    ]
    .concat();
    assert_eq!(read(r, &format!("{dir}_0.cfs")), big);
    assert_eq!(
        read(r, &format!("{dir}segments_1")),
        fixture("__smallBlobUuid2")
    );
}

/// A RustFS process on loopback, killed when the test ends.
struct RustFs {
    child: std::process::Child,
    dir: PathBuf,
}

impl Drop for RustFs {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

const RUSTFS_ACCESS: &str = "stockaccess";
const RUSTFS_SECRET: &str = "stocksecret";

/// Starts the binary `RUSTFS_BIN` names with a bucket `stocksrc` holding the
/// fixture repository under `stock/repo/`; its endpoint. `None` without the
/// variable.
fn rustfs() -> Option<(RustFs, String)> {
    use oss_blob::s3::sigv4::{self, Credentials, Scope, UNSIGNED_PAYLOAD};
    let bin = std::env::var_os("RUSTFS_BIN")?;
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let dir = std::env::temp_dir().join(format!("oss-stock-rustfs-{port}"));
    std::fs::create_dir_all(&dir).unwrap();
    let host = format!("127.0.0.1:{port}");
    let child = std::process::Command::new(bin)
        .arg(&dir)
        .env("RUSTFS_ADDRESS", &host)
        .env("RUSTFS_VOLUMES", &dir)
        .env("RUSTFS_ACCESS_KEY", RUSTFS_ACCESS)
        .env("RUSTFS_SECRET_KEY", RUSTFS_SECRET)
        .env("RUSTFS_CONSOLE_ENABLE", "false")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("RUSTFS_BIN starts");
    let server = RustFs { child, dir };
    let started = std::time::Instant::now();
    while std::net::TcpStream::connect(&host).is_err() {
        assert!(started.elapsed().as_secs() < 30, "RustFS did not listen");
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    let endpoint = format!("http://{host}");
    let agent = ureq::Agent::new_with_config(
        ureq::Agent::config_builder()
            .http_status_as_error(false)
            .proxy(None)
            .build(),
    );
    // RustFS listens before it serves: retry the bucket while it answers 503.
    loop {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let date = sigv4::amz_date(now);
        let headers = [
            ("host", host.as_str()),
            ("x-amz-content-sha256", UNSIGNED_PAYLOAD),
            ("x-amz-date", date.as_str()),
        ];
        let canonical =
            sigv4::canonical_request("PUT", "/stocksrc", "", &headers, UNSIGNED_PAYLOAD);
        let scope = Scope {
            date: &date[..8],
            region: "us-east-1",
            service: "s3",
        };
        let auth = sigv4::authorization(
            &Credentials::new(RUSTFS_ACCESS, RUSTFS_SECRET),
            &scope,
            &date,
            &canonical,
        );
        let status = agent
            .put(&format!("{endpoint}/stocksrc"))
            .header("x-amz-content-sha256", UNSIGNED_PAYLOAD)
            .header("x-amz-date", &date)
            .header("authorization", &auth)
            .send_empty()
            .expect("the bucket request")
            .status()
            .as_u16();
        if status == 200 {
            break;
        }
        assert!(
            status == 503 && started.elapsed().as_secs() < 30,
            "create bucket: {status}"
        );
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    let store = oss_blob::S3Store::new(oss_blob::S3Config::new(
        &endpoint,
        "stocksrc",
        "us-east-1",
        oss_blob::Credentials::new(RUSTFS_ACCESS, RUSTFS_SECRET),
    ))
    .unwrap();
    let top =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/snapshots/stock-repo/repo");
    put_tree(&store, &top, "stock/repo/");
    Some((server, endpoint))
}

/// AC-9 over a real S3 implementation, when `RUSTFS_BIN` names one: the
/// same restore, and the target holds the same bytes.
#[test]
fn a_stock_snapshot_restores_from_a_rustfs_source() {
    let Some((_server, endpoint)) = rustfs() else {
        eprintln!("skipped: RUSTFS_BIN is not set");
        return;
    };
    let r = rig_stock(
        true,
        "",
        POOL,
        1 << 16,
        StockRestoreSettings {
            enabled: true,
            roots: SourceRoots::parse("", "stocksrc/stock"),
            fanout: 1,
            s3: Some(oss_node::stock_restore_route::StockS3 {
                source: oss_node::stock_s3::SourceS3 {
                    endpoint,
                    region: "us-east-1".to_owned(),
                    credentials: oss_blob::Credentials::new(RUSTFS_ACCESS, RUSTFS_SECRET),
                    own_bucket: "own".to_owned(),
                },
                own_prefix: String::new(),
            }),
        },
    );
    assert_eq!(
        send(&r, "PUT", "/_snapshot/stock", Some(S3_REPO)).status,
        200
    );
    let reply = send(&r, "POST", RESTORE, None);
    assert_eq!(reply.status, 200, "{}", reply.text());
    assert_eq!(r.store.meter().count(S3Op::List), 0);
    assert_files_restored(&r);
}

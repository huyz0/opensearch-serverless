//! A RustFS process the proactive rig can run over: the evidence tests
//! name it with `RUSTFS_BIN`, and skip when it is not set.

use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use oss_blob::{BlobStore, Emulator, Rounds, Shared};
use oss_cluster::{LeaseConfig, Membership, SystemClock};
use oss_format::NodeLease;
use oss_meta::{DeleteMode, DescriptorStore};

use super::Cluster;
use oss_blob::s3::sigv4::{self, Credentials, Scope, UNSIGNED_PAYLOAD};
use oss_blob::s3::{S3Config, S3Store};

const ACCESS: &str = "proactiveaccess";
const SECRET: &str = "proactivesecret";
const BUCKET: &str = "proactive";
const REGION: &str = "us-east-1";

/// A RustFS process, killed with its data directory when dropped.
pub struct RustFs {
    child: Child,
    dir: PathBuf,
    endpoint: String,
}

impl Drop for RustFs {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .expect("a port")
        .local_addr()
        .expect("an address")
        .port()
}

impl RustFs {
    /// Starts the binary `RUSTFS_BIN` names and makes the bucket; `None`
    /// when the variable is not set.
    pub fn start() -> Option<Self> {
        let bin = std::env::var_os("RUSTFS_BIN")?;
        let dir = std::env::temp_dir().join(format!("oss-proactive-rustfs-{}", free_port()));
        std::fs::create_dir_all(&dir).expect("a directory");
        let host = format!("127.0.0.1:{}", free_port());
        let child = Command::new(bin)
            .arg(&dir)
            .env("RUSTFS_ADDRESS", &host)
            .env("RUSTFS_VOLUMES", &dir)
            .env("RUSTFS_ACCESS_KEY", ACCESS)
            .env("RUSTFS_SECRET_KEY", SECRET)
            .env("RUSTFS_CONSOLE_ENABLE", "false")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("RUSTFS_BIN starts");
        let me = Self {
            child,
            dir,
            endpoint: format!("http://{host}"),
        };
        let started = Instant::now();
        while TcpStream::connect(&host).is_err() {
            assert!(
                started.elapsed() < Duration::from_secs(30),
                "RustFS did not listen"
            );
            std::thread::sleep(Duration::from_millis(100));
        }
        create_bucket(&me.endpoint, &host);
        Some(me)
    }

    /// A client of the bucket; each call is a new client with its own meter.
    pub fn store(&self) -> Arc<dyn BlobStore> {
        let config = S3Config::new(
            &self.endpoint,
            BUCKET,
            REGION,
            Credentials::new(ACCESS, SECRET),
        );
        Arc::new(S3Store::new(config).expect("an S3 client"))
    }
}

/// Creates the bucket, retrying while the store answers 503: RustFS accepts
/// connections before it can serve requests.
fn create_bucket(endpoint: &str, host: &str) {
    let started = Instant::now();
    loop {
        let status = put_bucket(endpoint, host);
        if status != 503 || started.elapsed() > Duration::from_secs(30) {
            assert_eq!(status, 200, "create bucket");
            return;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

fn put_bucket(endpoint: &str, host: &str) -> u16 {
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("after the epoch")
        .as_secs();
    let date = sigv4::amz_date(now);
    let headers = [
        ("host", host),
        ("x-amz-content-sha256", UNSIGNED_PAYLOAD),
        ("x-amz-date", date.as_str()),
    ];
    let path = format!("/{BUCKET}");
    let canonical = sigv4::canonical_request("PUT", &path, "", &headers, UNSIGNED_PAYLOAD);
    let scope = Scope {
        date: &date[..8],
        region: REGION,
        service: "s3",
    };
    let auth = sigv4::authorization(&Credentials::new(ACCESS, SECRET), &scope, &date, &canonical);
    let agent = ureq::Agent::new_with_config(
        ureq::Agent::config_builder()
            .http_status_as_error(false)
            .proxy(None)
            .build(),
    );
    agent
        .put(&format!("{endpoint}{path}"))
        .header("x-amz-content-sha256", UNSIGNED_PAYLOAD)
        .header("x-amz-date", &date)
        .header("authorization", &auth)
        .send_empty()
        .expect("the bucket request")
        .status()
        .as_u16()
}

/// A cluster over a RustFS process, its nodes reaching it through a round
/// counter; `None` when `RUSTFS_BIN` is not set. Request counts and round
/// trips are real; the store's own latency is RustFS's.
pub fn cluster_rustfs_rounds() -> Option<Cluster> {
    cluster_rustfs(true)
}

/// As [`cluster_rustfs_rounds`], with the round counter only when `counted`.
pub fn cluster_rustfs(counted: bool) -> Option<Cluster> {
    let rustfs = RustFs::start()?;
    let store = rustfs.store();
    let membership = Arc::new(
        Membership::new(
            Arc::clone(&store),
            "",
            Arc::new(SystemClock),
            LeaseConfig {
                ttl_millis: 30_000,
                max_members: 16,
                conditional_deletes: false,
            },
        )
        .expect("membership"),
    );
    for (node, eph) in [("node-1", "e1"), ("node-2", "e2"), ("node-3", "e3")] {
        let mut lease = NodeLease::new(node, eph, "a", 0);
        lease.set_roles(["ingest".to_owned()]);
        membership.renew(&lease).expect("enrolled");
    }
    let rounds = counted.then(|| Arc::new(Rounds::new(Shared::new(Arc::clone(&store)))));
    let io: Arc<dyn BlobStore> = match &rounds {
        Some(rounds) => rounds.clone(),
        None => Arc::clone(&store),
    };
    let descriptors =
        Arc::new(DescriptorStore::new(io, "", DeleteMode::Tombstone).expect("descriptors"));
    Some(Cluster {
        store: Arc::new(Emulator::new(1 << 20, 1)),
        descriptors,
        rounds,
        external: Some(store),
        _rustfs: Some(rustfs),
    })
}

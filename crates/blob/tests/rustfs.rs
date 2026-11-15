//! The S3 client against a real S3 implementation: a RustFS binary on
//! loopback (docs/specs/s3-client/SPEC.md AC-14). It runs only when
//! `RUSTFS_BIN` names the binary, and says it skipped otherwise; nothing
//! here reaches beyond 127.0.0.1.
//!
//! RustFS is started with its documented environment (`RUSTFS_ADDRESS`,
//! `RUSTFS_ACCESS_KEY`, `RUSTFS_SECRET_KEY`, `RUSTFS_VOLUMES`), the bucket
//! is created with one signed `PUT /bucket`, and the scenario then runs
//! through `S3Store` alone.

use std::net::{TcpListener, TcpStream};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant, SystemTime};

use oss_blob::s3::sigv4::{self, Credentials, Scope, UNSIGNED_PAYLOAD};
use oss_blob::{
    BlobStore, ByteRange, CreateOutcome, ObjectMeta, S3Config, S3Op, S3Store, SliceSource,
};

const ACCESS: &str = "rustfsadmin";
const SECRET: &str = "rustfsadmin";
const BUCKET: &str = "oss-blob-test";
const REGION: &str = "us-east-1";

/// The RustFS process, killed when the test ends however it ends.
struct RustFs(Child);

impl Drop for RustFs {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// Creates the bucket with one signed request, as a provisioning step
/// outside the store's interface.
fn create_bucket(endpoint: &str, host: &str) {
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
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
    let response = agent
        .put(&format!("{endpoint}{path}"))
        .header("x-amz-content-sha256", UNSIGNED_PAYLOAD)
        .header("x-amz-date", &date)
        .header("authorization", &auth)
        .send_empty()
        .unwrap();
    assert_eq!(response.status().as_u16(), 200, "create bucket");
}

#[test]
fn the_client_round_trips_against_rustfs() {
    let Some(bin) = std::env::var_os("RUSTFS_BIN") else {
        eprintln!("skipped: set RUSTFS_BIN to a RustFS binary to run this test");
        return;
    };
    let dir = std::env::temp_dir().join(format!("oss-blob-rustfs-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let port = free_port();
    let host = format!("127.0.0.1:{port}");
    let _rustfs = RustFs(
        Command::new(bin)
            .arg(&dir)
            .env("RUSTFS_ADDRESS", &host)
            .env("RUSTFS_VOLUMES", &dir)
            .env("RUSTFS_ACCESS_KEY", ACCESS)
            .env("RUSTFS_SECRET_KEY", SECRET)
            .env("RUSTFS_CONSOLE_ENABLE", "false")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("RUSTFS_BIN starts"),
    );
    let started = Instant::now();
    while TcpStream::connect(&host).is_err() {
        assert!(
            started.elapsed() < Duration::from_secs(30),
            "RustFS did not listen"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
    let endpoint = format!("http://{host}");
    create_bucket(&endpoint, &host);

    let s3 = S3Store::new(S3Config::new(
        &endpoint,
        BUCKET,
        REGION,
        Credentials::new(ACCESS, SECRET),
    ))
    .unwrap();
    let body: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
    let mut buf = vec![0u8; 64 * 1024];
    s3.put("seg/a b+c", &mut SliceSource::new(&body), &mut buf)
        .unwrap();
    let mut got = Vec::new();
    s3.get("seg/a b+c", ByteRange::Full, &mut buf, &mut |c: &[u8]| {
        got.extend_from_slice(c);
        Ok(())
    })
    .unwrap();
    assert_eq!(got, body);
    got.clear();
    let range = ByteRange::From {
        start: 1000,
        len: 10,
    };
    let out = s3
        .get("seg/a b+c", range, &mut buf, &mut |c: &[u8]| {
            got.extend_from_slice(c);
            Ok(())
        })
        .unwrap();
    assert_eq!(
        (got.as_slice(), out.object_len),
        (&body[1000..1010], 300_000)
    );
    assert_eq!(
        s3.create_if_absent("log/1", &mut SliceSource::new(b"one"), &mut buf),
        Ok(CreateOutcome::Created)
    );
    assert_eq!(
        s3.create_if_absent("log/1", &mut SliceSource::new(b"two"), &mut buf),
        Ok(CreateOutcome::AlreadyExists)
    );
    assert_eq!(s3.head("log/1"), Ok(Some(ObjectMeta { len: 3 })));
    let mut keys = Vec::new();
    let page = s3
        // LIST: a test of the LIST page itself.
        .list_prefix("", None, 1000, &mut |k, n| keys.push((k.to_owned(), n)))
        .unwrap();
    assert!(!page.truncated);
    assert_eq!(
        keys,
        [("log/1".to_owned(), 3), ("seg/a b+c".to_owned(), 300_000)]
    );
    s3.delete("log/1").unwrap();
    assert_eq!(s3.head("log/1"), Ok(None));
    for (op, n) in [
        (S3Op::Put, 3),
        (S3Op::Get, 1),
        (S3Op::GetRange, 1),
        (S3Op::Head, 2),
        (S3Op::List, 1),
        (S3Op::Delete, 1),
    ] {
        assert_eq!(s3.meter().count(op), n, "{op:?}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

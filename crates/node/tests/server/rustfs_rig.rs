//! A RustFS process on loopback for the server tests that run against a real
//! S3 implementation. They run only when `RUSTFS_BIN` names the binary.

/// A RustFS process on loopback, killed when the test ends.
pub(crate) struct RustFs {
    child: std::process::Child,
    dir: std::path::PathBuf,
}

impl Drop for RustFs {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

pub(crate) const RUSTFS_ACCESS: &str = "stockaccess";
pub(crate) const RUSTFS_SECRET: &str = "stocksecret";

/// Starts the binary `RUSTFS_BIN` names with an empty bucket `bucket`; its
/// endpoint. `None` without the variable.
pub(crate) fn start(bucket: &str) -> Option<(RustFs, String)> {
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
            sigv4::canonical_request("PUT", &format!("/{bucket}"), "", &headers, UNSIGNED_PAYLOAD);
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
            .put(&format!("{endpoint}/{bucket}"))
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
    Some((server, endpoint))
}

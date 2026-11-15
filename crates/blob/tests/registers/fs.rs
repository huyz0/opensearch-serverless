//! AC-6 and AC-7: the filesystem store reads the Java shell's register and
//! takes the Java's lock.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use oss_blob::{BlobStore, FsStore, SwapOutcome};
use oss_testkit::TempDir;

use crate::read;

/// The environment variable that turns the test binary into a lock holder.
const HOLD: &str = "OSS_REGISTER_LOCK_HOLD";

/// The reserved directory the Java keeps its lock files in.
const LOCKS: &str = ".opensearch-register-locks#";

// AC-6
#[test]
fn a_register_the_java_shell_wrote_reads_with_its_generation() {
    let fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/shell-blobs/indices/fixture");
    #[allow(clippy::disallowed_methods, reason = "a test reads its fixture whole")]
    let bytes = std::fs::read(&fixture).unwrap();
    let dir = TempDir::new("registers-fixture");
    std::fs::create_dir_all(dir.path().join("indices")).unwrap();
    std::fs::copy(&fixture, dir.path().join("indices/fixture")).unwrap();
    let store = FsStore::new(dir.path()).unwrap();
    let (generation, value) = read(&store, "indices/fixture").unwrap();
    assert_eq!(
        generation,
        u64::from_be_bytes(bytes[..8].try_into().unwrap())
    );
    assert!(crate::is_initial(generation), "{generation:#x}");
    assert_eq!(value, &bytes[8..]);
    assert!(value.starts_with(b"{\"name\":\"fixture\""));
    assert_eq!(
        store.swap_register("indices/fixture", generation, b"{}"),
        Ok(SwapOutcome::Applied(generation + 1))
    );
}

/// The Java's `Math.floorMod(path.toString().hashCode(), 64)`.
fn java_stripe(path: &str) -> u32 {
    let hash = path
        .encode_utf16()
        .fold(0i32, |h, u| h.wrapping_mul(31).wrapping_add(i32::from(u)));
    hash.rem_euclid(64) as u32
}

/// The stripe file the Java locks for the register `name` in `dir`.
fn stripe_file(dir: &Path, name: &str) -> PathBuf {
    let real = dir.canonicalize().unwrap();
    let stripe = java_stripe(&real.join(name).to_string_lossy());
    real.join(LOCKS).join(stripe.to_string())
}

/// Not a test of its own: run by the test below as a child process, it
/// takes the lock Java's `FileChannel.lock` takes on the file named in
/// `HOLD`, says so, and holds it until its stdin closes.
#[cfg(unix)]
#[test]
fn lock_holder() {
    let Some(path) = std::env::var_os(HOLD) else {
        return;
    };
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
        .unwrap();
    rustix::fs::fcntl_lock(&file, rustix::fs::FlockOperation::LockExclusive).unwrap();
    println!("locked");
    std::io::stdout().flush().unwrap();
    let mut line = String::new();
    let _ = std::io::stdin().read_line(&mut line);
    drop(file);
}

// AC-7
#[cfg(unix)]
#[test]
fn a_swap_waits_for_another_processes_posix_lock() {
    let dir = TempDir::new("registers-lock");
    let store = std::sync::Arc::new(FsStore::new(dir.path()).unwrap());
    let SwapOutcome::Applied(g) = store.create_register("idx/reg", b"v0").unwrap() else {
        panic!("create refused");
    };
    let lock = stripe_file(&dir.path().join("idx"), "reg");
    assert!(
        lock.is_file(),
        "the create took the Java's stripe file {lock:?}"
    );

    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "fs::lock_holder",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(HOLD, &lock)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut out = BufReader::new(child.stdout.take().unwrap());
    let mut line = String::new();
    while !line.contains("locked") {
        line.clear();
        assert!(
            out.read_line(&mut line).unwrap() > 0,
            "the child exited first"
        );
    }

    let (done, finished) = mpsc::sync_channel(1);
    let swapper = std::sync::Arc::clone(&store);
    let swap = std::thread::spawn(move || {
        let outcome = swapper.swap_register("idx/reg", g, b"v1");
        done.send(()).unwrap();
        outcome
    });
    assert!(
        finished.recv_timeout(Duration::from_millis(500)).is_err(),
        "the swap went ahead while another process held the lock"
    );
    drop(child.stdin.take());
    assert_eq!(swap.join().unwrap(), Ok(SwapOutcome::Applied(g + 1)));
    assert!(child.wait().unwrap().success());
    assert_eq!(read(&*store, "idx/reg"), Some((g + 1, b"v1".to_vec())));
}

// AC-7: a different register's stripe is not held up.
#[test]
fn registers_in_one_directory_use_their_own_stripes() {
    let dir = TempDir::new("registers-stripes");
    let store = FsStore::new(dir.path()).unwrap();
    for name in ["a", "b", "c", "\u{fc}n\u{ef}c\u{f6}d\u{e9}"] {
        store.create_register(&format!("d/{name}"), b"v").unwrap();
        assert!(stripe_file(&dir.path().join("d"), name).is_file(), "{name}");
    }
    // Staged files are gone, and listings skip the lock directory.
    let mut keys = Vec::new();
    // LIST: the operation under test.
    store
        .list_prefix("d/", None, 10, &mut |k, _| keys.push(k.to_owned()))
        .unwrap();
    assert_eq!(keys.len(), 4, "{keys:?}");
    let staged = std::fs::read_dir(dir.path().join("d").join(LOCKS))
        .unwrap()
        .filter(|e| {
            e.as_ref()
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("pending")
        })
        .count();
    assert_eq!(staged, 0);
}

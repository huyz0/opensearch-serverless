//! docs/specs/stock-snapshot-export AC-5: a shard's committed files are
//! copied, each as one `__<id>` blob under the shard's directory of the
//! target; length, CRC and Lucene version are measured on the way, and a
//! file that does not match its record fails the copy.

use std::sync::Arc;

use oss_blob::{BlobStore, ByteRange, Emulator, ExportStore};
use oss_format::{
    FileLocation, ShardCapture, ShardRef, StockEnvelopeLimits, StockShardSnapshot,
    stock_blob_to_json,
};
use oss_meta::{CopyBuffers, ExportError, ExportTarget, SnapshotTaker};

const REPO: &str = "../../fixtures/snapshots/stock/repo/indices/M3IuI3tpQ7mcpIwUIA1kfQ/0/";
const BASE: &str = "b/";

#[allow(clippy::disallowed_methods)] // ARITH: a test reads a recorded fixture.
fn blob(file: &str) -> Vec<u8> {
    let path = format!("{}/{REPO}{file}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

/// The recorded shard's four files as (physical name, bytes, listed checksum).
fn recorded() -> Vec<(String, Vec<u8>, String)> {
    let mut json = Vec::new();
    stock_blob_to_json(
        &blob("snap-q6tlFJQcRUS97oE_eOaE7Q.dat"),
        "snapshot",
        &StockEnvelopeLimits::default(),
        &mut json,
    )
    .unwrap();
    let snap = StockShardSnapshot::parse(&json).unwrap();
    snap.files
        .iter()
        .map(|f| {
            let bytes = f.inline.clone().unwrap_or_else(|| blob(&f.name));
            (f.physical_name.clone(), bytes, f.checksum.clone())
        })
        .collect()
}

fn put(store: &dyn BlobStore, key: &str, bytes: &[u8]) {
    let mut body = oss_blob::SliceSource::new(bytes);
    store.put(key, &mut body, &mut [0u8; 64]).expect("put");
}

fn read(store: &dyn BlobStore, key: &str) -> Vec<u8> {
    let mut out = Vec::new();
    store
        .get(key, ByteRange::Full, &mut [0u8; 64], &mut |c: &[u8]| {
            out.extend_from_slice(c);
            Ok(())
        })
        .expect("get");
    out
}

struct Fixture {
    taker: SnapshotTaker,
    native: Arc<Emulator>,
    target: Arc<Emulator>,
    capture: ShardCapture,
}

fn fixture(tamper: impl Fn(&str, &mut Vec<u8>)) -> Fixture {
    let native = Arc::new(Emulator::new(1 << 20, 1));
    let target = Arc::new(Emulator::new(1 << 20, 1));
    let from = ShardRef {
        index: "logs",
        uuid: "U1",
        shard: 0,
    };
    let whole = FileLocation::Whole { term_dir: "t1" };
    let mut files = Vec::new();
    let mut lengths = Vec::new();
    for (name, mut bytes, _) in recorded() {
        tamper(&name, &mut bytes);
        let (key, _) = from.blob_key(BASE, &name, &whole).unwrap();
        put(&*native, &key, &bytes);
        files.push((name.clone(), whole.encode()));
        lengths.push((name, bytes.len() as u64));
    }
    Fixture {
        taker: SnapshotTaker::new(native.clone(), BASE),
        native,
        target,
        capture: ShardCapture {
            shard: 0,
            term: 1,
            files,
            lengths,
        },
    }
}

fn export(fx: &Fixture) -> Result<Vec<oss_meta::ExportedFile>, ExportError> {
    let store = ExportStore::new(fx.target.clone(), "repo/");
    let to = ExportTarget {
        store: &store,
        prefix: "repo/",
    };
    let from = ShardRef {
        index: "logs",
        uuid: "U1",
        shard: 0,
    };
    let mut read_buf = vec![0u8; 64];
    let mut write_buf = vec![0u8; 64];
    let mut buffers = CopyBuffers {
        read: &mut read_buf,
        write: &mut write_buf,
    };
    fx.taker
        .export_shard(from, &fx.capture, &to, "IDX", &mut buffers)
}

#[test]
fn the_recorded_shard_is_copied_with_its_listed_checksums_and_versions() {
    let fx = fixture(|_, _| {});
    let done = export(&fx).unwrap();
    let listed = recorded();
    assert_eq!(done.len(), listed.len());
    for (file, (physical, bytes, sum)) in done.iter().zip(&listed) {
        assert_eq!(&file.physical_name, physical);
        assert_eq!(&file.checksum, sum, "{physical}");
        assert_eq!(file.length, bytes.len() as u64);
        assert_eq!(file.written_by, "10.5.1", "{physical}");
        assert!(file.name.starts_with("__"));
        let key = format!("repo/indices/IDX/0/{}", file.name);
        assert_eq!(&read(&*fx.target, &key), bytes, "{physical}");
    }
    let mut names: Vec<_> = done.iter().map(|f| &f.name).collect();
    names.sort();
    names.dedup();
    assert_eq!(names.len(), listed.len(), "every blob name is its own");
}

#[test]
fn a_file_whose_bytes_changed_fails_the_copy() {
    let fx = fixture(|name, bytes| {
        if name.ends_with(".cfs") {
            bytes[0] ^= 1;
        }
    });
    assert!(matches!(export(&fx), Err(ExportError::Mismatch(_))));
}

#[test]
fn a_file_shorter_than_recorded_fails_the_copy() {
    let mut fx = fixture(|_, _| {});
    for (_, length) in &mut fx.capture.lengths {
        *length += 1;
    }
    assert!(export(&fx).is_err());
}

#[test]
fn a_capture_of_another_shard_or_a_file_without_a_length_is_refused() {
    let mut fx = fixture(|_, _| {});
    fx.capture.shard = 1;
    assert!(matches!(export(&fx), Err(ExportError::Unsupported(_))));
    fx.capture.shard = 0;
    fx.capture.lengths.clear();
    assert!(matches!(export(&fx), Err(ExportError::Unsupported(_))));
}

#[test]
fn a_file_with_no_version_to_take_is_refused() {
    let mut fx = fixture(|_, _| {});
    // Without the segment info and the segments file, nothing says a version.
    fx.capture
        .files
        .retain(|(n, _)| !n.ends_with(".si") && !n.starts_with("segments_"));
    assert!(matches!(export(&fx), Err(ExportError::Unsupported(_))));
}

#[test]
fn only_the_ranged_reads_and_the_creates_are_issued() {
    let fx = fixture(|_, _| {});
    let before = fx.native.meter().total();
    export(&fx).unwrap();
    assert!(fx.native.meter().total() > before);
    // The target saw creates only: no LIST, no delete.
    assert_eq!(fx.target.meter().count(oss_blob::S3Op::List), 0);
    assert_eq!(fx.target.meter().count(oss_blob::S3Op::Delete), 0);
}

/// A Lucene file: `body` plus a footer with the right CRC.
fn lucene(body: &[u8]) -> Vec<u8> {
    let mut file = body.to_vec();
    file.extend_from_slice(&[0xC0, 0x28, 0x93, 0xE8, 0, 0, 0, 0]);
    let crc = crc32fast::hash(&file);
    file.extend_from_slice(&u64::from(crc).to_be_bytes());
    file
}

/// An index header with codec `codec`, header version 0, a zero id, no suffix.
fn header(codec: &str) -> Vec<u8> {
    let mut h = vec![0x3f, 0xd7, 0x6c, 0x17, u8::try_from(codec.len()).unwrap()];
    h.extend_from_slice(codec.as_bytes());
    h.extend_from_slice(&[0, 0, 0, 0]);
    h.extend_from_slice(&[0u8; 16]);
    h.push(0);
    h
}

fn segment_info(major: u32, minor: u32, bugfix: u32) -> Vec<u8> {
    let mut b = header("Lucene90SegmentInfo");
    for n in [major, minor, bugfix] {
        b.extend_from_slice(&n.to_le_bytes());
    }
    lucene(&b)
}

fn segments_file(major: u8, minor: u8, bugfix: u8) -> Vec<u8> {
    let mut b = header("segments");
    b.extend_from_slice(&[major, minor, bugfix]);
    lucene(&b)
}

#[test]
fn each_file_is_versioned_by_its_own_segment() {
    let native = Arc::new(Emulator::new(1 << 20, 1));
    let target = Arc::new(Emulator::new(1 << 20, 1));
    let from = ShardRef {
        index: "logs",
        uuid: "U1",
        shard: 0,
    };
    let whole = FileLocation::Whole { term_dir: "t1" };
    let members: Vec<(&str, Vec<u8>)> = vec![
        ("_0.si", segment_info(10, 5, 1)),
        ("_0.cfs", lucene(b"zero")),
        ("_1.si", segment_info(9, 12, 0)),
        ("_1.cfs", lucene(b"one")),
        ("_1_2.liv", lucene(b"live")),
        ("segments_4", segments_file(10, 5, 1)),
    ];
    let mut files = Vec::new();
    let mut lengths = Vec::new();
    for (name, bytes) in &members {
        let (key, _) = from.blob_key(BASE, name, &whole).unwrap();
        put(&*native, &key, bytes);
        files.push(((*name).to_owned(), whole.encode()));
        lengths.push(((*name).to_owned(), bytes.len() as u64));
    }
    let fx = Fixture {
        taker: SnapshotTaker::new(native.clone(), BASE),
        native,
        target,
        capture: ShardCapture {
            shard: 0,
            term: 1,
            files,
            lengths,
        },
    };
    let done = export(&fx).unwrap();
    let got: Vec<(&str, &str)> = done
        .iter()
        .map(|f| (f.physical_name.as_str(), f.written_by.as_str()))
        .collect();
    assert_eq!(
        got,
        [
            ("_0.si", "10.5.1"),
            ("_0.cfs", "10.5.1"),
            ("_1.si", "9.12.0"),
            ("_1.cfs", "9.12.0"),
            ("_1_2.liv", "9.12.0"),
            ("segments_4", "10.5.1"),
        ]
    );
}

#[test]
fn an_error_says_why() {
    assert!(ExportError::Unsupported("x").to_string().contains('x'));
    let e = export(&{
        let mut fx = fixture(|_, _| {});
        fx.capture.lengths.clear();
        fx
    })
    .unwrap_err();
    assert!(e.to_string().contains("no recorded length"));
}

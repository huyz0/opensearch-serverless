//! docs/specs/stock-snapshot-restore AC-4 and AC-5: a stock index's files
//! are copied whole into the restored index, a virtual file from its inline
//! bytes; a file whose bytes do not match its listing fails the restore.

use std::sync::Arc;

use oss_blob::{BlobStore, ByteRange, Emulator};
use oss_format::{FileLocation, ManifestEntry, RestoreIndex, ShardRef, StockFile, encode_manifest};
use oss_meta::{
    CopyBuffers, DeleteMode, DescriptorStore, RestoreError, SnapshotRestorer, StockShardCopy,
    StockSource,
};

const SOURCE: &str = "src/";

/// A Lucene file of `body` bytes plus footer, and its CRC in base 36.
fn lucene(body: &[u8]) -> (Vec<u8>, String) {
    let mut file = body.to_vec();
    file.extend_from_slice(&[0xC0, 0x28, 0x93, 0xE8, 0, 0, 0, 0]);
    let crc = crc32fast::hash(&file);
    file.extend_from_slice(&u64::from(crc).to_be_bytes());
    let mut n = u64::from(crc);
    let mut digits = Vec::new();
    loop {
        digits.push(b"0123456789abcdefghijklmnopqrstuvwxyz"[(n % 36) as usize]);
        n /= 36;
        if n == 0 {
            break;
        }
    }
    digits.reverse();
    (file, String::from_utf8(digits).unwrap())
}

fn listed(name: &str, physical: &str, bytes: &[u8], sum: &str, part: Option<u64>) -> StockFile {
    StockFile {
        name: name.into(),
        physical_name: physical.into(),
        length: bytes.len() as u64,
        checksum: sum.into(),
        part_size: part,
        written_by: None,
        inline: None,
    }
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

struct World {
    src: Arc<dyn BlobStore>,
    dst: Arc<dyn BlobStore>,
    big: Vec<u8>,
    small: Vec<u8>,
    files: Vec<StockFile>,
}

/// A source with a file of three chunks, an unsplit file and a virtual one.
fn world() -> World {
    let src: Arc<dyn BlobStore> = Arc::new(Emulator::new(1 << 20, 1));
    let dst: Arc<dyn BlobStore> = Arc::new(Emulator::new(1 << 20, 1));
    let (big, big_sum) = lucene(&(0..90u8).collect::<Vec<_>>());
    let (small, small_sum) = lucene(b"small");
    let (virt, virt_sum) = lucene(b"v");
    let dir = format!("{SOURCE}indices/IDX/0/");
    // 106 bytes in chunks of 40: 40, 40, 26.
    for (i, chunk) in big.chunks(40).enumerate() {
        put(&*src, &format!("{dir}__big.part{i}"), chunk);
    }
    put(&*src, &format!("{dir}__small"), &small);
    let mut v = listed("v__x", "_2.si", &virt, &virt_sum, None);
    v.inline = Some(virt);
    let files = vec![
        listed("__big", "_0.cfs", &big, &big_sum, Some(40)),
        listed("__small", "_1.si", &small, &small_sum, None),
        v,
    ];
    World {
        src,
        dst,
        big,
        small,
        files,
    }
}

fn restore(w: &World, files: &[StockFile], chunk: usize) -> Result<(), RestoreError> {
    let descriptors =
        DescriptorStore::new(Arc::clone(&w.dst), "", DeleteMode::Conditional).unwrap();
    let native = RestoreIndex {
        name: "logs".into(),
        uuid: "OLD".into(),
        number_of_shards: 1,
        mapping: None,
        settings: Some(br#"{"index.refresh_interval":"5s"}"#.to_vec()),
        shards: Vec::new(),
    };
    let from = StockSource {
        store: &*w.src,
        prefix: SOURCE,
    };
    let copy = [StockShardCopy {
        index_id: "IDX",
        shard: 0,
        files,
    }];
    let (mut r, mut wr) = (vec![0u8; chunk], vec![0u8; chunk]);
    SnapshotRestorer::new(Arc::clone(&w.dst), "").restore_stock_index(
        &descriptors,
        &from,
        &native,
        &copy,
        "logs2",
        "NEW",
        1,
        &mut vec![0u8; 4096],
        &mut CopyBuffers {
            read: &mut r,
            write: &mut wr,
        },
    )
}

fn key_of(name: &str) -> String {
    let to = ShardRef {
        index: "logs2",
        uuid: "NEW",
        shard: 0,
    };
    to.blob_key("", name, &FileLocation::Whole { term_dir: "t=1" })
        .unwrap()
        .0
}

#[test]
fn every_file_lands_whole_at_any_buffer_size() {
    for chunk in [7, 40, 64, 4096] {
        let w = world();
        restore(&w, &w.files, chunk).unwrap_or_else(|e| panic!("buffer {chunk}: {e}"));
        assert_eq!(read(&*w.dst, &key_of("_0.cfs")), w.big);
        assert_eq!(read(&*w.dst, &key_of("_1.si")), w.small);
        assert_eq!(
            read(&*w.dst, &key_of("_2.si")),
            w.files[2].inline.clone().unwrap()
        );
        let to = ShardRef {
            index: "logs2",
            uuid: "NEW",
            shard: 0,
        };
        let whole = FileLocation::Whole { term_dir: "t=1" }.encode();
        let entries = [
            ("_0.cfs", w.big.len()),
            ("_1.si", w.small.len()),
            ("_2.si", w.files[2].length as usize),
        ]
        .map(|(name, length)| ManifestEntry {
            name,
            location: whole.clone(),
            length: length as u64,
        });
        // A register holds a small header before its value.
        let stored = read(&*w.dst, &to.manifest_key("").unwrap());
        let expected = encode_manifest(1, None, &entries, -1, -1).unwrap();
        assert!(
            stored.ends_with(&expected),
            "the manifest landed with its files"
        );
    }
}

#[test]
fn a_flipped_byte_in_a_chunk_fails_the_restore() {
    let w = world();
    let key = format!("{SOURCE}indices/IDX/0/__big.part1");
    let mut chunk = read(&*w.src, &key);
    chunk[3] ^= 1;
    put(&*w.src, &key, &chunk);
    assert!(restore(&w, &w.files, 16).is_err());
}

#[test]
fn a_missing_or_short_chunk_fails_the_restore() {
    let w = world();
    let key = format!("{SOURCE}indices/IDX/0/__big.part2");
    put(&*w.src, &key, b"short");
    assert!(restore(&w, &w.files, 16).is_err(), "short");
    let w = world();
    let mut gone = w.files.clone();
    gone[0].name = "__nothere".into();
    assert!(restore(&w, &gone, 16).is_err(), "missing");
}

#[test]
fn a_wrong_length_or_listed_checksum_fails_the_restore() {
    let w = world();
    let mut longer = w.files.clone();
    longer[1].length += 1;
    assert!(restore(&w, &longer, 16).is_err(), "length");
    let w = world();
    let mut sum = w.files.clone();
    sum[1].checksum = "zzzzzz".into();
    assert!(restore(&w, &sum, 16).is_err(), "checksum");
    let w = world();
    let mut virt = w.files.clone();
    virt[2].length += 1;
    assert!(restore(&w, &virt, 16).is_err(), "virtual length");
}

#[test]
fn a_name_that_is_not_a_blob_name_is_refused_before_any_read() {
    let w = world();
    let mut bad = w.files.clone();
    bad[1].name = "../x".into();
    assert!(restore(&w, &bad, 16).is_err());
    // A virtual file reads no key, so only the check refuses its name.
    let w = world();
    let mut bad = w.files.clone();
    bad[2].name = "v__a/b".into();
    assert!(restore(&w, &bad, 16).is_err(), "virtual name");
    let w = world();
    let mut bad = w.files.clone();
    bad[1].physical_name = "a/b".into();
    assert!(restore(&w, &bad, 16).is_err());
}

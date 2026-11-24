//! The restore's pieces against an in-process store: where a ranged GET
//! starts and ends, how a stream is cut into files, and what is refused.

use lucene_store::{ByteBuffersDirectory, Directory};
use oss_blob::{BlobStore, Emulator, SliceSource};

use super::*;

fn store_with(key: &str, bytes: &[u8]) -> Emulator {
    let st = Emulator::new(1 << 20, 3);
    st.put(key, &mut SliceSource::new(bytes), &mut [0u8; 16])
        .expect("put");
    st
}

fn member(name: &str, offset: u64, length: Option<u64>) -> Member {
    Member {
        // ALLOC: test data, a few bytes.
        name: name.to_owned(),
        offset,
        length,
    }
}

fn file(dir: &ByteBuffersDirectory, name: &str) -> Vec<u8> {
    // ALLOC: test data, a few bytes.
    dir.open(name).expect("the file exists").to_vec()
}

fn pattern(n: u8) -> Vec<u8> {
    // ALLOC: test data, a few bytes.
    (0..n).collect()
}

#[test]
fn files_are_cut_from_a_stream_that_arrives_in_small_chunks_skipping_the_gaps() {
    let st = store_with("blob", &pattern(20));
    let dir = ByteBuffersDirectory::new();
    let members = [member("a", 2, Some(4)), member("b", 8, Some(4))];
    spread(&st, "blob", &members, &dir, &mut [0u8; 3]).expect("spread");
    assert_eq!(file(&dir, "a"), [2, 3, 4, 5]);
    assert_eq!(file(&dir, "b"), [8, 9, 10, 11]);
    assert_eq!(st.meter().count(oss_blob::S3Op::GetRange), 1);
    assert_eq!(st.meter().count(oss_blob::S3Op::Get), 0);
}

#[test]
fn files_of_no_bytes_are_created_without_a_request() {
    let st = store_with("blob", &pattern(4));
    let dir = ByteBuffersDirectory::new();
    let before = st.meter().total();
    let members = [member("a", 0, Some(0)), member("b", 0, Some(0))];
    spread(&st, "blob", &members, &dir, &mut [0u8; 8]).expect("spread");
    assert_eq!(st.meter().total(), before, "no GET for no bytes");
    assert!(file(&dir, "a").is_empty());
    assert!(file(&dir, "b").is_empty());
}

#[test]
fn a_blob_that_ends_inside_the_last_file_is_short_by_that_file() {
    let st = store_with("blob", &pattern(6));
    let dir = ByteBuffersDirectory::new();
    let members = [member("a", 0, Some(4)), member("b", 4, Some(6))];
    let err = spread(&st, "blob", &members, &dir, &mut [0u8; 8]).expect_err("short");
    assert!(matches!(&err, RestoreError::Short(n) if n == "b"), "{err}");
}

#[test]
fn a_blob_that_ends_before_a_file_starts_is_short_by_the_first_file() {
    let st = store_with("blob", &pattern(6));
    let dir = ByteBuffersDirectory::new();
    let members = [member("a", 100, Some(4))];
    let err = spread(&st, "blob", &members, &dir, &mut [0u8; 8]).expect_err("short");
    assert!(matches!(&err, RestoreError::Short(n) if n == "a"), "{err}");
}

#[test]
fn a_whole_blob_of_one_file_of_unknown_length_ends_with_the_stream() {
    let st = store_with("blob", &pattern(10));
    let dir = ByteBuffersDirectory::new();
    let members = [member("a", 0, None)];
    spread(&st, "blob", &members, &dir, &mut [0u8; 3]).expect("spread");
    assert_eq!(file(&dir, "a"), pattern(10));
    assert_eq!(st.meter().count(oss_blob::S3Op::Get), 1);
}

#[test]
fn a_known_length_last_file_the_stream_did_not_fill_is_not_finished() {
    let st = store_with("blob", &pattern(10));
    let dir = ByteBuffersDirectory::new();
    // The second file is last and has a length; the blob ends 2 bytes in.
    let members = [member("a", 0, Some(4)), member("b", 4, Some(8))];
    assert!(matches!(
        spread(&st, "blob", &members, &dir, &mut [0u8; 3]),
        Err(RestoreError::Short(n)) if n == "b"
    ));
}

#[test]
fn a_manifest_that_fits_the_first_buffer_is_read_in_one_request() {
    let st = Emulator::new(1 << 20, 3);
    st.create_register("m", b"{}").expect("created");
    let before = st.meter().total();
    let (value, generation) = read_manifest(&st, "m").expect("read").expect("there");
    assert_eq!(value, b"{}");
    assert!(generation > 0);
    assert_eq!(st.meter().total() - before, 1);
    assert!(read_manifest(&st, "none").expect("read").is_none());
}

#[test]
fn every_restore_error_says_what_failed() {
    // ALLOC: test data, a few bytes.
    let text = |e: RestoreError| e.to_string();
    assert_eq!(
        // ALLOC: test data, a few bytes.
        text(RestoreError::Directory("full".to_owned())),
        "writing a restored file: full"
    );
    assert_eq!(
        // ALLOC: test data, a few bytes.
        text(RestoreError::Short("_0.cfs".to_owned())),
        "the blob of [_0.cfs] ends before the file does"
    );
    assert_eq!(text(RestoreError::TooLarge), "a manifest too large to read");
    assert!(text(RestoreError::Blob(BlobError::InvalidRange)).starts_with("reading the commit: "));
}

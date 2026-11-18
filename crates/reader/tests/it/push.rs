//! Opening the commit a writer pushed.

use oss_blob::{ByteRange, S3Op};
use oss_format::Register;
use oss_reader::{ReaderError, ShardFiles, open_shard_pushed};

use crate::{BASE, INDEX, cache, counts, since, store_with_fixtures};

const REGISTER: &str = "segments/fixture#fixture-uuid-0000000#0/manifest";

/// The commit register's value, as a writer pushes it.
fn pushed(store: &oss_blob::PooledStore<oss_blob::Emulator>) -> Vec<u8> {
    let mut bytes = Vec::new();
    store
        .get(REGISTER, ByteRange::Full, &mut |chunk: &[u8]| {
            bytes.extend_from_slice(chunk);
            Ok(())
        })
        .expect("read the register");
    Register::parse(&bytes).expect("register").value.to_vec()
}

#[test]
fn a_pushed_commit_is_found_with_the_descriptor_read_alone() {
    let store = store_with_fixtures();
    let value = pushed(&store);
    let before = counts(&store);
    let (files, meta) = ShardFiles::open_pushed_with_meta(&store, BASE, INDEX, 0, &value)
        .expect("open from the push");
    assert_eq!(since(&store, before), [(S3Op::Get, 1)]);
    assert_eq!(meta.number_of_shards, 1);
    let from_register = ShardFiles::open(&store, BASE, INDEX, 0).expect("open");
    assert_eq!(files, from_register, "the same commit, the same files");
}

#[test]
fn the_engine_opens_a_pushed_commit_with_one_whole_read() {
    let store = store_with_fixtures();
    let value = pushed(&store);
    let before = counts(&store);
    let shard = open_shard_pushed(store.clone(), cache(), BASE, INDEX, 0, &value).expect("open");
    assert_eq!(shard.reader.max_doc(), 5);
    let made = since(&store, before);
    let count = |op: S3Op| made.iter().find(|(o, _)| *o == op).map_or(0, |(_, n)| *n);
    assert_eq!(count(S3Op::Get), 1, "the descriptor only: {made:?}");
    assert!(count(S3Op::GetRange) >= 1, "{made:?}");
    for op in [S3Op::List, S3Op::Head, S3Op::Put, S3Op::Copy, S3Op::Delete] {
        assert_eq!(count(op), 0, "{op:?}: {made:?}");
    }
}

#[test]
fn a_push_that_is_not_a_commit_is_refused_after_the_descriptor() {
    let store = store_with_fixtures();
    let err = ShardFiles::open_pushed_with_meta(&store, BASE, INDEX, 0, b"nonsense")
        .expect_err("not a commit");
    assert!(matches!(err, ReaderError::Format(_)), "{err:?}");
}

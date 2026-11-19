//! Opening a shard from a point in time's frozen commit.

use oss_blob::{ByteRange, S3Op};
use oss_format::{CommitManifest, PitFile, PitShard, PointInTime, Register, ShardRef};
use oss_reader::{ReaderError, ShardFiles, open_frozen_shard};

use crate::{BASE, INDEX, cache, counts, since, store_with_fixtures};

type Store = oss_blob::PooledStore<oss_blob::Emulator>;

/// A view of shard 0 of the fixture index as capture would record it.
fn view(store: &Store, uuid: Option<&str>) -> PointInTime {
    let (_, meta) = ShardFiles::open_with_meta(store, BASE, INDEX, 0).expect("open");
    let at = ShardRef {
        index: INDEX,
        uuid: &meta.uuid,
        shard: 0,
    };
    let mut bytes = Vec::new();
    store
        .get(
            &at.manifest_key(BASE).expect("key"),
            ByteRange::Full,
            &mut |chunk: &[u8]| {
                bytes.extend_from_slice(chunk);
                Ok(())
            },
        )
        .expect("read the register");
    let manifest =
        CommitManifest::parse(Register::parse(&bytes).expect("register").value).expect("manifest");
    PointInTime {
        id: "AAAAAAAAAAAAAAAAAAAAAA".to_owned(),
        index: INDEX.to_owned(),
        index_uuid: uuid.map(str::to_owned),
        expires_at_millis: u64::MAX / 2,
        capturing: false,
        shards: vec![PitShard {
            shard: 0,
            term: manifest.term,
            files: manifest
                .files()
                .iter()
                .map(|f| PitFile {
                    name: f.name.to_string(),
                    location: f.location.encode(),
                    length: f.length,
                })
                .collect(),
        }],
    }
}

fn uuid(store: &Store) -> String {
    ShardFiles::open_with_meta(store, BASE, INDEX, 0)
        .expect("open")
        .1
        .uuid
}

#[test]
fn a_frozen_commit_resolves_like_the_live_one_with_one_get() {
    let store = store_with_fixtures();
    let live = ShardFiles::open(&store, BASE, INDEX, 0).expect("live");
    let view = view(&store, Some(&uuid(&store)));
    let before = counts(&store);
    let (frozen, meta) = ShardFiles::open_frozen(&store, BASE, INDEX, &view, 0).expect("frozen");
    assert_eq!(since(&store, before), [(S3Op::Get, 1)]);
    assert_eq!(meta.number_of_shards, 1);
    assert_eq!(frozen.term, live.term);
    assert_eq!(
        frozen.stage(),
        &oss_reader::commit_push::Stage::new(live.term, None, -1),
        "a frozen commit was staged by no writer"
    );
    assert_eq!(
        frozen.names().collect::<Vec<_>>(),
        live.names().collect::<Vec<_>>()
    );
    for name in live.names() {
        assert_eq!(frozen.get(name), live.get(name), "{name}");
    }
}

#[test]
fn the_engine_opens_a_frozen_commit_without_the_commit_register() {
    let store = store_with_fixtures();
    let view = view(&store, Some(&uuid(&store)));
    let before = counts(&store);
    let shard =
        open_frozen_shard(store.clone(), cache(), BASE, INDEX, &view, 0).expect("open frozen");
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
fn a_view_of_another_incarnation_is_not_held_and_one_without_a_uuid_is() {
    let store = store_with_fixtures();
    let other = view(&store, Some("some-earlier-incarnation"));
    let err = ShardFiles::open_frozen(&store, BASE, INDEX, &other, 0).expect_err("other");
    assert!(matches!(err, ReaderError::NotHeld), "{err}");
    let nameless = view(&store, None);
    ShardFiles::open_frozen(&store, BASE, INDEX, &nameless, 0).expect("by name");
}

#[test]
fn a_view_still_capturing_or_without_the_shard_is_not_frozen() {
    let store = store_with_fixtures();
    let good = view(&store, None);
    let capturing = PointInTime {
        capturing: true,
        ..good.clone()
    };
    let err = ShardFiles::open_frozen(&store, BASE, INDEX, &capturing, 0).expect_err("capturing");
    assert!(matches!(err, ReaderError::NotFrozen(0)), "{err}");
    let err = ShardFiles::open_frozen(&store, BASE, INDEX, &good, 1).expect_err("shard 1");
    assert!(matches!(err, ReaderError::NotFrozen(1)), "{err}");
}

#[test]
fn a_file_without_a_length_or_a_followable_location_is_refused() {
    let store = store_with_fixtures();
    let mut good = view(&store, None);
    good.shards[0].files[0].length = None;
    let err = ShardFiles::open_frozen(&store, BASE, INDEX, &good, 0).expect_err("no length");
    assert!(matches!(err, ReaderError::UnknownLength(_)), "{err}");
    let mut bad = view(&store, None);
    bad.shards[0].files[0].location = "../escape".to_owned();
    let err = ShardFiles::open_frozen(&store, BASE, INDEX, &bad, 0).expect_err("location");
    assert!(matches!(err, ReaderError::Format(_)), "{err}");
}

#[test]
fn a_missing_index_is_not_found_after_one_get() {
    let store = store_with_fixtures();
    let view = view(&store, None);
    let before = counts(&store);
    let err = ShardFiles::open_frozen(&store, BASE, "no-such-index", &view, 0).expect_err("gone");
    assert!(
        matches!(err, ReaderError::Blob(oss_blob::BlobError::NotFound)),
        "{err}"
    );
    assert_eq!(since(&store, before), [(S3Op::Get, 1)]);
}

#[test]
fn a_frozen_open_costs_what_a_second_live_open_does_on_a_shared_cache() {
    let store = store_with_fixtures();
    let view = view(&store, Some(&uuid(&store)));
    let shared = cache();
    let first =
        oss_reader::open_shard(store.clone(), shared.clone(), BASE, INDEX, 0).expect("live");
    let before = counts(&store);
    let second =
        oss_reader::open_shard(store.clone(), shared.clone(), BASE, INDEX, 0).expect("live");
    let live_again = since(&store, before);
    let before = counts(&store);
    let frozen = open_frozen_shard(store.clone(), shared, BASE, INDEX, &view, 0).expect("frozen");
    let made = since(&store, before);
    assert_eq!(frozen.reader.max_doc(), first.reader.max_doc());
    assert_eq!(second.reader.max_doc(), first.reader.max_doc());
    // The frozen open makes the live reopen's requests less the commit
    // register it never reads.
    let gets = |v: &[(S3Op, u64)]| v.iter().find(|(o, _)| *o == S3Op::Get).map_or(0, |x| x.1);
    let ranges = |v: &[(S3Op, u64)]| {
        v.iter()
            .find(|(o, _)| *o == S3Op::GetRange)
            .map_or(0, |x| x.1)
    };
    assert_eq!(
        ranges(&made),
        ranges(&live_again),
        "{made:?} / {live_again:?}"
    );
    assert_eq!(
        gets(&made) + 1,
        gets(&live_again),
        "{made:?} / {live_again:?}"
    );
}

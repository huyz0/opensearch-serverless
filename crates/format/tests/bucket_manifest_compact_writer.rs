//! docs/specs/bucket-manifest-compact-writer/SPEC.md, AC-1 to AC-6.

mod bucket_support;

use bucket_support::{DEFLATE, File, LZ4, Len, Loc, Member, NONE, ZSTD, compact, sample_commit};
use oss_format::{
    BucketManifest, BucketMember, FormatError, MAX_BUCKET_MEMBERS, ManifestCodec, ManifestEntry,
    encode_manifest, parse_bucket_member,
};

fn commit() -> bucket_support::Commit {
    let mut commit = sample_commit();
    commit.extra.clear();
    commit.digest = None;
    commit.files.push(File {
        name: ".hidden",
        loc: Loc::Prefix("t=3"),
        len: Len::Given(1),
    });
    commit.files.retain(|f| !matches!(f.loc, Loc::Raw(_)));
    // The shell writes a length that equals its range as the range's; the
    // sample's test-encoder form (given) is one it never writes.
    for file in &mut commit.files {
        if file.name == "_0.si" {
            file.len = Len::OfRange;
        }
    }
    commit
}

fn members() -> Vec<Member> {
    vec![
        Member {
            uuid: "aaaa".to_owned(),
            index: "alpha".to_owned(),
            version: 3,
            commit: Some(commit()),
        },
        Member {
            uuid: "mmmm".to_owned(),
            index: "mu-é-日-😀".to_owned(),
            version: 1,
            commit: Some(commit()),
        },
        Member {
            uuid: "zzzz".to_owned(),
            index: "empty".to_owned(),
            version: 0,
            commit: None,
        },
    ]
}

const CODECS: [(u8, ManifestCodec); 3] = [
    (NONE, ManifestCodec::None),
    (DEFLATE, ManifestCodec::Deflate),
    (ZSTD, ManifestCodec::Zstd),
];

#[test]
fn every_codec_round_trips_and_the_member_reader_agrees() {
    let java = compact(NONE, "bk-1", 11, &members());
    let manifest = BucketManifest::parse(&java).unwrap();
    for (_, codec) in CODECS {
        let bytes = manifest.encode_compact(codec).unwrap();
        assert_eq!(BucketManifest::parse(&bytes).unwrap(), manifest);
        for (uuid, member) in manifest.members() {
            let single = parse_bucket_member(&bytes, uuid).unwrap();
            assert_eq!((single.bucket.as_str(), single.term), ("bk-1", 11));
            assert_eq!(single.member.as_ref(), Some(member));
        }
    }
}

#[test]
fn the_body_is_what_the_shell_writes_for_the_same_manifest() {
    for (id, codec) in CODECS {
        let java = compact(id, "bk-1", 11, &members());
        let manifest = BucketManifest::parse(&java).unwrap();
        let ours = manifest.encode_compact(codec).unwrap();
        // Every codec is checked on its body: the compressors differ in bytes.
        let plain = compact(NONE, "bk-1", 11, &members());
        assert_eq!(
            BucketManifest::parse(&ours).unwrap(),
            BucketManifest::parse(&plain).unwrap()
        );
        if id == NONE {
            assert_eq!(ours, java);
        }
    }
}

#[test]
fn parse_reads_every_codec_including_lz4() {
    let none = BucketManifest::parse(&compact(NONE, "bk-1", 11, &members())).unwrap();
    for id in [DEFLATE, ZSTD, LZ4] {
        let bytes = compact(id, "bk-1", 11, &members());
        assert_eq!(BucketManifest::parse(&bytes).unwrap(), none);
    }
    assert_eq!(none.len(), 3);
    assert_eq!(none.member("zzzz").unwrap().version, 0);
}

fn noisy_member(i: usize) -> BucketMember {
    let mut state = 0x9E37_79B9_7F4A_7C15_u64 ^ (i as u64);
    let name: String = (0..1000)
        .map(|_| {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            char::from(u8::try_from(state >> 57).unwrap())
        })
        .collect();
    BucketMember::first(&name, None).unwrap()
}

fn commit_json(term: u64) -> Vec<u8> {
    let files: Vec<ManifestEntry<'_>> = (0..40)
        .map(|n| ManifestEntry {
            name: Box::leak(format!("_{n}.cfs").into_boxed_str()),
            location: format!("t={term}/s{n}#0:100"),
            length: 100,
        })
        .collect();
    encode_manifest(term, Some("nodeA"), &files, 5, -1).unwrap()
}

#[test]
fn auto_compresses_only_a_large_manifest_that_shrinks() {
    let codec_byte = |bytes: &[u8]| bytes[5];
    let small = BucketManifest::empty("b")
        .unwrap()
        .with_member("u", BucketMember::first("i", Some(commit_json(2))).unwrap())
        .unwrap();
    assert_eq!(
        codec_byte(&small.encode_compact_auto(ManifestCodec::Zstd).unwrap()),
        0
    );

    let mut large = BucketManifest::empty("b").unwrap();
    for i in 0..200 {
        let member = BucketMember::first("idx", Some(commit_json(i + 1))).unwrap();
        large = large.with_member(&format!("uuid-{i:04}"), member).unwrap();
    }
    let bytes = large.encode_compact_auto(ManifestCodec::Zstd).unwrap();
    assert_eq!(codec_byte(&bytes), 2);
    assert_eq!(BucketManifest::parse(&bytes).unwrap(), large);
    let plain = large.encode_compact(ManifestCodec::None).unwrap();
    assert!(plain.len() >= 32 * 1024 && bytes.len() * 5 <= plain.len() * 4);

    let mut noisy = BucketManifest::empty("b").unwrap();
    for i in 0..40 {
        noisy = noisy
            .with_member(&format!("uuid-{i:04}"), noisy_member(i))
            .unwrap();
    }
    let bytes = noisy.encode_compact_auto(ManifestCodec::Zstd).unwrap();
    assert_eq!(codec_byte(&bytes), 0, "7-bit noise saves under a fifth");
    assert_eq!(BucketManifest::parse(&bytes).unwrap(), noisy);
    // A codec that is asked for is used whatever it saves.
    assert_eq!(
        codec_byte(&noisy.encode_compact(ManifestCodec::Zstd).unwrap()),
        2
    );
}

#[test]
fn every_proper_prefix_of_a_compact_manifest_is_an_error() {
    for (id, _) in CODECS {
        let bytes = compact(id, "bk-1", 11, &members());
        for end in 0..bytes.len() {
            assert!(
                BucketManifest::parse(&bytes[..end]).is_err(),
                "codec {id} prefix {end}"
            );
        }
    }
}

#[test]
fn too_many_members_are_refused_on_read() {
    let many: Vec<Member> = (0..=MAX_BUCKET_MEMBERS)
        .map(|i| Member {
            uuid: format!("u{i:05}"),
            index: "i".to_owned(),
            version: 0,
            commit: None,
        })
        .collect();
    let bytes = compact(NONE, "bk-1", 1, &many);
    assert!(matches!(
        BucketManifest::parse(&bytes),
        Err(FormatError::TooLarge { .. })
    ));
}

#[test]
fn an_unknown_codec_and_a_body_with_trailing_bytes_are_refused() {
    let mut bytes = compact(NONE, "bk-1", 11, &members());
    bytes[5] = 9;
    assert!(matches!(
        BucketManifest::parse(&bytes),
        Err(FormatError::Unsupported { .. })
    ));
    let mut raw = bucket_support::body("bk-1", 11, &members());
    raw.push(0);
    let framed = bucket_support::frame(NONE, raw.len(), &raw);
    assert!(BucketManifest::parse(&framed).is_err());
}

#[test]
fn a_string_past_1024_characters_is_refused_on_write() {
    let long = "x".repeat(1025);
    let manifest = BucketManifest::empty("b")
        .unwrap()
        .with_member("u", BucketMember::first(&long, None).unwrap())
        .unwrap();
    assert!(manifest.encode_compact(ManifestCodec::None).is_err());
    let ok = "x".repeat(1024);
    let manifest = BucketManifest::empty("b")
        .unwrap()
        .with_member("u", BucketMember::first(&ok, None).unwrap())
        .unwrap();
    assert!(manifest.encode_compact(ManifestCodec::None).is_ok());
}

//! Key derivation, JSON text, Java's modified UTF-8, registers and messages.

// ARITH: sizes and offsets here are fixture lengths and small constants; an overflow
// panics the test in a debug build, which fails it as it should.
#![allow(clippy::arithmetic_side_effects)]

use std::borrow::Cow;

use oss_format::{
    CommitManifest, DescriptorRecord, FormatError, NodePackHeader, Register, ShardRef,
    descriptor_key, node_pack_format,
};

#[test]
fn keys_carry_the_base() {
    assert_eq!(
        descriptor_key("deploy/", "logs").unwrap(),
        "deploy/indices/logs"
    );
    let shard = ShardRef {
        index: "logs",
        uuid: "u1",
        shard: 12,
    };
    assert_eq!(
        shard.base_path("deploy/").unwrap(),
        "deploy/segments/logs#u1#12/"
    );
    assert_eq!(
        shard.manifest_key("").unwrap(),
        "segments/logs#u1#12/manifest"
    );
    let manifest = CommitManifest::parse(
        br#"{"term":7,"files":{"_3.si":"t=7","_3.cfs":"t=5/_3.seg#40:9","_3.cfe":"@n9/e=00000000000000000002/00000000000000000003#1:2"},"lengths":{"_3.si":4}}"#,
    )
    .unwrap();
    let extents = shard.resolve("d/", &manifest).unwrap();
    let flat: Vec<(&str, &str, u64, Option<u64>)> = extents
        .iter()
        .map(|e| (e.name.as_ref(), e.key.as_str(), e.offset, e.length))
        .collect();
    assert_eq!(
        flat,
        [
            (
                "_3.cfe",
                "d/packs/n9/e=00000000000000000002/00000000000000000003",
                1,
                Some(2)
            ),
            ("_3.cfs", "d/segments/logs#u1#12/t=5/_3.seg", 40, Some(9)),
            ("_3.si", "d/segments/logs#u1#12/t=7/_3.si", 0, Some(4)),
        ]
    );
}

#[test]
fn keys_refuse_parts_that_would_escape_or_collide() {
    let corrupt = |r: Result<String, FormatError>| matches!(r, Err(FormatError::Corrupt { .. }));
    assert!(
        corrupt(descriptor_key("deploy", "logs")),
        "a base ends in /"
    );
    for name in ["", ".", "..", "a/b"] {
        assert!(corrupt(descriptor_key("", name)), "{name:?}");
    }
    for (index, uuid) in [("a#b", "u"), ("a", "u#1"), ("", "u"), ("a", "..")] {
        assert!(
            corrupt(
                ShardRef {
                    index,
                    uuid,
                    shard: 0
                }
                .base_path("")
            ),
            "{index:?} {uuid:?}"
        );
    }
    let shard = ShardRef {
        index: "a",
        uuid: "u",
        shard: 0,
    };
    for files in [
        r#"{"..":"t=1"}"#,
        r#"{"x":".."}"#,
        r#"{"x":"t=1/../y#0:1"}"#,
        r#"{"x":"t=1/a//b#0:1"}"#,
        r#"{"x":"../t=1/y#0:1"}"#,
    ] {
        let json = format!(r#"{{"term":1,"files":{files}}}"#);
        let manifest = CommitManifest::parse(json.as_bytes());
        let resolved = manifest.and_then(|m| shard.resolve("", &m).map(|_| ()));
        assert!(
            matches!(resolved, Err(FormatError::Corrupt { .. })),
            "{files}"
        );
    }
}

#[test]
fn json_strings_decode_escapes() {
    let json = r#"{"name":"a\"b\\c\/d\b\f\n\r\t\u00e9\ud83d\ude00","uuid":"plain-é","number_of_shards":1}"#;
    let DescriptorRecord::Index(d) = DescriptorRecord::parse(json.as_bytes()).unwrap() else {
        panic!()
    };
    assert_eq!(d.name, "a\"b\\c/d\u{8}\u{c}\n\r\té😀");
    assert!(matches!(d.name, Cow::Owned(_)));
    assert!(matches!(d.uuid, Cow::Borrowed("plain-é")));
    for bad in [
        r#"{"name":"\ud83d","uuid":"u","number_of_shards":1}"#,
        r#"{"name":"\ud83dx","uuid":"u","number_of_shards":1}"#,
        r#"{"name":"\ud83d\u0041","uuid":"u","number_of_shards":1}"#,
        r#"{"name":"\ude00","uuid":"u","number_of_shards":1}"#,
        r#"{"name":"\x","uuid":"u","number_of_shards":1}"#,
        r#"{"name":"\u00g0","uuid":"u","number_of_shards":1}"#,
        r#"{"name":"\u00","uuid":"u","number_of_shards":1}"#,
        r#"{"name":"\u00"#,
        r#"{"name":"n","x":[1,2"#,
        "{\"name\":\"a\nb\",\"uuid\":\"u\",\"number_of_shards\":1}",
        "{\"name\":\"a\\n\nb\",\"uuid\":\"u\",\"number_of_shards\":1}",
        r#"{"name":"n","uuid":"u","number_of_shards":01}"#,
        r#"{"name":"n","uuid":"u","number_of_shards":-}"#,
        r#"{"name":"n","uuid":"u","number_of_shards":1,"x":1.}"#,
        r#"{"name":"n","uuid":"u","number_of_shards":1,"x":1e}"#,
        r#"{"name":"n","uuid":"u","number_of_shards":1,"x":tru}"#,
        r#"{"name":"n","uuid":"u","number_of_shards":1,"x":trux}"#,
        r#"{"name":"n","uuid":"u","number_of_shards":1,"x":fals0}"#,
        r#"{"name":"n","uuid":"u","number_of_shards":1,"x":nul"#,
        r#"{"name":"n","uuid":"u","number_of_shards":1,"x":[1 2]}"#,
        r#"{"name":"n","uuid":"u","number_of_shards":1,"x":[1,]}"#,
        r#"{"name":"n","uuid":"u","number_of_shards":1,"x":@}"#,
        r#"{"name":"n" "uuid":"u"}"#,
        r#"{"name" "n"}"#,
        r#"{name:"n"}"#,
        r#"{"name":"n","uuid":"u","number_of_shards":1,"x":"\"#,
    ] {
        assert!(
            matches!(
                DescriptorRecord::parse(bad.as_bytes()),
                Err(FormatError::Corrupt { .. })
            ),
            "{bad}"
        );
    }
    // Scalars of every kind are skipped, and numbers past i64 or with a
    // fraction or exponent are numbers too.
    let skipped = r#" { "x" : [ true , false , null , -0 , 1.5e+3 , 2E-1 , 99999999999999999999 , "s" , { } , [ ] ] ,
        "name":"n","uuid":"u","number_of_shards":1 } "#;
    assert!(DescriptorRecord::parse(skipped.as_bytes()).is_ok());
    // Spaces and other printable bytes need no escape.
    let DescriptorRecord::Index(d) =
        DescriptorRecord::parse(br#"{"name":"a b ~","uuid":"u","number_of_shards":1}"#).unwrap()
    else {
        panic!()
    };
    assert_eq!(d.name, "a b ~");
    // A number past i64 where an integer belongs is refused.
    assert!(CommitManifest::parse(br#"{"term":99999999999999999999,"files":{}}"#).is_err());
}

fn pack_named(node: &[u8]) -> Result<String, FormatError> {
    let mut body = Vec::new();
    body.extend_from_slice(&u16::try_from(node.len()).unwrap().to_be_bytes());
    body.extend_from_slice(node);
    body.extend_from_slice(&0i64.to_be_bytes());
    body.extend_from_slice(&0i64.to_be_bytes());
    body.extend_from_slice(&0i32.to_be_bytes());
    let mut header = node_pack_format::MAGIC.to_be_bytes().to_vec();
    header.push(node_pack_format::VERSION);
    header.extend_from_slice(&i32::try_from(body.len()).unwrap().to_be_bytes());
    header.extend_from_slice(&body);
    NodePackHeader::parse(&header).map(|h| h.node.into_owned())
}

#[test]
fn pack_strings_are_modified_utf8() {
    assert_eq!(pack_named(b"node-\xc3\xa9").unwrap(), "node-é");
    // NUL as two bytes, a raw NUL, and a character past U+FFFF as two surrogates.
    assert_eq!(pack_named(b"a\xc0\x80b\x00").unwrap(), "a\0b\0");
    assert_eq!(pack_named(b"\xed\xa0\xbd\xed\xb8\x80").unwrap(), "😀");
    // Two- and three-byte characters beside a NUL decode on the slow path too.
    assert_eq!(pack_named(b"\xc3\xa9\xe2\x82\xac\xc0\x80").unwrap(), "é€\0");
    for bad in [
        &b"\xf0\x9f\x98\x80"[..],
        b"\xed\xa0\xbd",
        b"\xc3",
        b"\xc3\x41",
        b"\xe2\x82",
        b"\x80",
        b"\xf8",
    ] {
        assert!(
            matches!(pack_named(bad), Err(FormatError::Corrupt { .. })),
            "{bad:?}"
        );
    }
}

#[test]
fn registers_split_generation_and_value() {
    let register = Register::parse(&[0, 0, 0, 1, 0, 0, 0, 2, b'{', b'}']).unwrap();
    assert_eq!(
        (register.generation, register.value),
        ((1 << 32) | 2, &b"{}"[..])
    );
    assert_eq!(Register::parse(&[0; 8]).unwrap().value, b"");
    assert_eq!(
        Register::parse(&[0; 7]),
        Err(FormatError::Truncated {
            what: "register",
            needed: 8,
            have: 7
        })
    );
}

#[test]
fn errors_read_as_sentences() {
    let cases = [
        (
            FormatError::Truncated {
                what: "x",
                needed: 9,
                have: 3,
            },
            "x is truncated: needs 9 bytes, has 3",
        ),
        (
            FormatError::Truncated {
                what: "x",
                needed: 0,
                have: 3,
            },
            "x is truncated at 3 bytes",
        ),
        (
            FormatError::Corrupt {
                what: "x",
                reason: "bad",
            },
            "malformed x: bad",
        ),
        (
            FormatError::Unsupported {
                what: "x",
                version: 4,
            },
            "x of unknown format version 4",
        ),
        (
            FormatError::TooLarge {
                what: "x",
                limit: 1,
                actual: 2,
            },
            "x too large: 2, limit 1",
        ),
        (
            FormatError::NotFollowable {
                what: "x",
                reason: "why",
            },
            "x cannot be followed: why",
        ),
    ];
    for (error, text) in cases {
        assert_eq!(error.to_string(), text);
    }
}

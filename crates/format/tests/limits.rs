//! Every bounded resource, at its limit and one past it.

// ARITH: sizes and offsets here are fixture lengths and small constants; an overflow
// panics the test in a debug build, which fails it as it should.
#![allow(clippy::arithmetic_side_effects)]

use oss_format::{
    CommitManifest, DescriptorRecord, FormatError, NodePackHeader, SegmentBlobHeader,
    manifest_format, node_pack_format, segment_blob_format,
};

fn segment_prefix(length: u32) -> Vec<u8> {
    let mut prefix = b"SEG1".to_vec();
    prefix.extend_from_slice(&length.to_be_bytes());
    prefix
}

#[test]
fn segment_blob_header_length_limit() {
    let at = segment_blob_format::MAX_HEADER_BYTES;
    assert_eq!(
        segment_blob_format::header_end(&segment_prefix(at)),
        Ok(8 + at as usize)
    );
    assert_eq!(
        segment_blob_format::header_end(&segment_prefix(at + 1)),
        Err(FormatError::TooLarge {
            what: "segment blob header",
            limit: at.into(),
            actual: u64::from(at) + 1
        })
    );
    // A Java int that went negative reads as past the limit.
    assert!(matches!(
        segment_blob_format::header_end(&segment_prefix(u32::MAX)),
        Err(FormatError::TooLarge { .. })
    ));
}

fn segment_blob_with_files(count: usize) -> Vec<u8> {
    // Every file sits after the header, which stays under 64 KiB at 1,025 files.
    let mut json = String::from(r#"{"version":1,"files":{"#);
    for i in 0..count {
        if i > 0 {
            json.push(',');
        }
        json.push_str(&format!(r#""f{i}":[{},1]"#, 1_000_000 + i));
    }
    json.push_str("}}");
    let mut blob = segment_prefix(u32::try_from(json.len()).unwrap());
    blob.extend_from_slice(json.as_bytes());
    blob
}

#[test]
fn segment_blob_file_count_limit() {
    let at = segment_blob_format::MAX_FILES;
    assert_eq!(
        SegmentBlobHeader::parse(&segment_blob_with_files(at)).map(|h| h.files().len()),
        Ok(at)
    );
    assert_eq!(
        SegmentBlobHeader::parse(&segment_blob_with_files(at + 1)),
        Err(FormatError::TooLarge {
            what: "segment blob header",
            limit: at as u64,
            actual: at as u64 + 1
        })
    );
}

fn pack_prefix(length: i32) -> Vec<u8> {
    let mut prefix = node_pack_format::MAGIC.to_be_bytes().to_vec();
    prefix.push(node_pack_format::VERSION);
    prefix.extend_from_slice(&length.to_be_bytes());
    prefix
}

#[test]
fn node_pack_header_length_limit() {
    let at = node_pack_format::MAX_HEADER_BYTES;
    let at_signed = i32::try_from(at).unwrap();
    assert_eq!(
        node_pack_format::header_end(&pack_prefix(at_signed)),
        Ok(9 + at as usize)
    );
    assert_eq!(
        node_pack_format::header_end(&pack_prefix(at_signed + 1)),
        Err(FormatError::TooLarge {
            what: "node pack header",
            limit: at.into(),
            actual: u64::from(at) + 1
        })
    );
    assert!(matches!(
        node_pack_format::header_end(&pack_prefix(-1)),
        Err(FormatError::Corrupt { .. })
    ));
}

/// A pack header claiming more blocks than its bytes could hold is refused
/// before anything is allocated for them: a block takes at least 26 bytes.
#[test]
fn node_pack_block_count_is_bounded_by_the_header() {
    let header = |count: i32, trailing: usize| {
        let mut body = Vec::new();
        body.extend_from_slice(&1u16.to_be_bytes());
        body.push(b'n');
        body.extend_from_slice(&0i64.to_be_bytes());
        body.extend_from_slice(&0i64.to_be_bytes());
        body.extend_from_slice(&count.to_be_bytes());
        body.resize(body.len() + trailing, 0);
        let mut header = pack_prefix(i32::try_from(body.len()).unwrap());
        header.extend_from_slice(&body);
        NodePackHeader::parse(&header).map(|h| (h.blocks().len(), h.pack_length() == h.body_start))
    };
    let too_many = |limit, actual| {
        Err(FormatError::TooLarge {
            what: "node pack header",
            limit,
            actual,
        })
    };
    assert_eq!(header(i32::MAX, 0), too_many(0, i32::MAX as u64));
    assert_eq!(header(3, 52), too_many(2, 3));
    assert_eq!(header(2, 51), too_many(1, 2));
    // At the bound the count passes, and the zero bytes then fail as empty strings.
    assert!(matches!(header(2, 52), Err(FormatError::Corrupt { .. })));
    assert_eq!(header(0, 0), Ok((0, true)));
}

#[test]
fn format_constants_are_the_javas() {
    assert_eq!(node_pack_format::MAX_HEADER_BYTES, 4_194_304);
    assert_eq!(node_pack_format::PREFIX_BYTES, 9);
    assert_eq!(node_pack_format::MAGIC, 0x4E50_4B31);
    assert_eq!(segment_blob_format::PREFIX_BYTES, 8);
    assert_eq!(segment_blob_format::MAX_HEADER_BYTES, 65_536);
    assert_eq!(segment_blob_format::MAGIC, *b"SEG1");
}

fn manifest_with_files(count: usize) -> String {
    let mut json = String::from(r#"{"term":1,"files":{"#);
    for i in 0..count {
        if i > 0 {
            json.push(',');
        }
        json.push_str(&format!(r#""f{i}":"t=1""#));
    }
    json.push_str("}}");
    json
}

fn manifest_with_lengths(count: usize) -> String {
    let mut json = String::from(r#"{"term":1,"files":{},"lengths":{"#);
    for i in 0..count {
        if i > 0 {
            json.push(',');
        }
        json.push_str(&format!(r#""f{i}":1"#));
    }
    json.push_str("}}");
    json
}

#[test]
fn manifest_file_count_limit() {
    let at = manifest_format::MAX_FILES;
    let too_large = Err(FormatError::TooLarge {
        what: "commit manifest",
        limit: at as u64,
        actual: at as u64 + 1,
    });
    let files = manifest_with_files(at);
    assert_eq!(
        CommitManifest::parse(files.as_bytes()).map(|m| m.files().len()),
        Ok(at)
    );
    assert_eq!(
        CommitManifest::parse(manifest_with_files(at + 1).as_bytes()).map(|_| ()),
        too_large
    );
    assert!(CommitManifest::parse(manifest_with_lengths(at).as_bytes()).is_ok());
    assert_eq!(
        CommitManifest::parse(manifest_with_lengths(at + 1).as_bytes()).map(|_| ()),
        too_large
    );
}

fn nested(depth: usize) -> String {
    // The record's own object is the first level.
    let inner = depth - 1;
    format!(
        r#"{{"name":"n","uuid":"u","number_of_shards":1,"deep":{}{}}}"#,
        "[".repeat(inner),
        "]".repeat(inner)
    )
}

#[test]
fn json_nesting_limit() {
    // 64 levels are skipped; the 65th is refused, so skipping never recurses without bound.
    assert!(matches!(
        DescriptorRecord::parse(nested(64).as_bytes()),
        Ok(DescriptorRecord::Index(_))
    ));
    assert_eq!(
        DescriptorRecord::parse(nested(65).as_bytes()),
        Err(FormatError::Corrupt {
            what: "index descriptor",
            reason: "the JSON nests too deeply"
        })
    );
}

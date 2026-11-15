//! Decoding bytes the Java shell wrote (fixtures/shell-blobs).

// ARITH: sizes and offsets here are fixture lengths and small constants; an overflow
// panics the test in a debug build, which fails it as it should.
#![allow(clippy::arithmetic_side_effects)]

mod common;

use common::*;
use oss_format::{
    CommitManifest, DescriptorRecord, FileLocation, NodePackHeader, Register, SegmentBlobHeader,
    ShardRef, descriptor_key, node_pack_format, node_pack_key, segment_blob_format,
};

fn manifest_bytes() -> Vec<u8> {
    blob(&manifest_key())
}

#[test]
fn descriptor_names_the_uuid_and_shards() {
    let key = descriptor_key(BASE, INDEX).unwrap();
    assert_eq!(key, "indices/fixture");
    let bytes = blob(&key);
    let register = Register::parse(&bytes).unwrap();
    let DescriptorRecord::Index(descriptor) = DescriptorRecord::parse(register.value).unwrap()
    else {
        panic!("an index");
    };
    assert_eq!(descriptor.name, INDEX);
    assert_eq!(descriptor.uuid, UUID);
    assert_eq!(descriptor.number_of_shards, 1);
    assert_eq!(descriptor.bucket, None);
    assert!(!descriptor.has_unit_layout);
}

#[test]
fn manifest_reads_every_field() {
    let bytes = manifest_bytes();
    let register = Register::parse(&bytes).unwrap();
    assert!(register.generation > 0);
    let manifest = CommitManifest::parse(register.value).unwrap();
    assert_eq!(manifest.term, 2);
    assert_eq!(manifest.writer.as_deref(), Some(SECOND_NODE));
    assert_eq!(manifest.wal_ordinal, 0);
    assert_eq!(manifest.stage_sequence, 1);
    assert!(manifest.has_digest);
    let names: Vec<&str> = manifest.files().iter().map(|f| f.name.as_ref()).collect();
    assert_eq!(
        names,
        [
            "_0.cfe",
            "_0.cfs",
            "_0.si",
            "_1.cfe",
            "_1.cfs",
            "_1.si",
            "segments_5"
        ]
    );
    let si = manifest.file("_0.si").unwrap();
    assert_eq!(
        si.location,
        FileLocation::NodePacked {
            node: FIRST_NODE,
            epoch: PACK_EPOCH,
            ordinal: 0,
            offset: 538,
            length: 333
        }
    );
    assert_eq!(si.length, Some(333));
    let segments = manifest.file("segments_5").unwrap();
    assert_eq!(
        segments.location,
        FileLocation::Packed {
            term_dir: "t=2",
            blob: "_1.seg",
            offset: 445,
            length: 397
        }
    );
    assert!(manifest.file("_2.si").is_none());
}

/// From two register values to every file's bytes, with keys derived only.
#[test]
fn resolved_extents_hold_whole_lucene_files() {
    let manifest_blob = manifest_bytes();
    let manifest = CommitManifest::parse(Register::parse(&manifest_blob).unwrap().value).unwrap();
    let shard = ShardRef {
        index: INDEX,
        uuid: UUID,
        shard: 0,
    };
    assert_eq!(shard.manifest_key(BASE).unwrap(), manifest_key());
    let extents = shard.resolve(BASE, &manifest).unwrap();
    assert_eq!(extents.len(), 7);
    for extent in &extents {
        let expected_key = if extent.name.starts_with("_0") {
            pack_key()
        } else {
            segment_blob_key()
        };
        assert_eq!(extent.key, expected_key, "{}", extent.name);
        let bytes = blob(&extent.key);
        let start = usize::try_from(extent.offset).unwrap();
        let end = start + usize::try_from(extent.length.unwrap()).unwrap();
        assert!(end <= bytes.len(), "{} ends inside its blob", extent.name);
        assert_lucene_file(&bytes[start..end], &extent.name);
    }
    assert_eq!(
        node_pack_key(BASE, FIRST_NODE, PACK_EPOCH, 0).unwrap(),
        pack_key()
    );
}

#[test]
fn segment_blob_header_agrees_with_the_manifest() {
    let bytes = blob(&segment_blob_key());
    let end = segment_blob_format::header_end(&bytes[..segment_blob_format::PREFIX_BYTES]).unwrap();
    let header = SegmentBlobHeader::parse(&bytes[..end]).unwrap();
    assert_eq!(header.header_end(), u64::try_from(end).unwrap());
    assert_eq!(header.blob_length(), u64::try_from(bytes.len()).unwrap());
    let names: Vec<&str> = header.files().iter().map(|f| f.name.as_ref()).collect();
    // Smallest first, then by name.
    assert_eq!(names, ["_1.si", "segments_5", "_1.cfe", "_1.cfs"]);
    let first = &header.files()[0];
    assert_eq!(
        first.offset,
        header.header_end(),
        "the first file follows the header"
    );
    for pair in header.files().windows(2) {
        assert_eq!(
            pair[0].offset + pair[0].length,
            pair[1].offset,
            "files are contiguous"
        );
    }
    let manifest_blob = manifest_bytes();
    let manifest = CommitManifest::parse(Register::parse(&manifest_blob).unwrap().value).unwrap();
    for file in header.files() {
        let recorded = manifest.file(&file.name).unwrap();
        assert_eq!(
            recorded.location.range(),
            Some((file.offset, file.length)),
            "{}",
            file.name
        );
    }
}

#[test]
fn node_pack_blocks_hold_segment_blobs_the_manifest_points_into() {
    let bytes = blob(&pack_key());
    let end = node_pack_format::header_end(&bytes[..node_pack_format::PREFIX_BYTES]).unwrap();
    let header = NodePackHeader::parse(&bytes[..end]).unwrap();
    assert_eq!(header.node, FIRST_NODE);
    assert_eq!(header.epoch, PACK_EPOCH);
    assert_eq!(header.ordinal, 0);
    assert_eq!(header.version, node_pack_format::VERSION);
    assert_eq!(header.pack_length(), u64::try_from(bytes.len()).unwrap());
    let [block] = header.blocks() else {
        panic!("one block: {:?}", header.blocks())
    };
    assert_eq!(block.index_uuid, UUID);
    assert_eq!(block.index_name, INDEX);
    assert_eq!(block.shard, 0);
    assert_eq!(block.name, "_0.seg");
    assert_eq!(block.offset, header.body_start);

    // The block is a segment blob exactly as it would be on its own.
    let start = usize::try_from(block.offset).unwrap();
    let inner = &bytes[start..start + usize::try_from(block.length).unwrap()];
    let segment = SegmentBlobHeader::parse(inner).unwrap();
    assert_eq!(segment.blob_length(), block.length);
    let manifest_blob = manifest_bytes();
    let manifest = CommitManifest::parse(Register::parse(&manifest_blob).unwrap().value).unwrap();
    let mut followed = 0;
    for file in manifest.files() {
        let FileLocation::NodePacked { offset, length, .. } = file.location else {
            continue;
        };
        let holder = header.block_containing(offset, length).unwrap();
        assert_eq!(holder, block);
        let in_blob = segment.file(&file.name).unwrap();
        assert_eq!(
            (block.offset + in_blob.offset, in_blob.length),
            (offset, length),
            "{}",
            file.name
        );
        followed += 1;
    }
    assert_eq!(followed, 3);
    // The first commit's segments file rode in the block, and the second commit dropped it.
    assert!(
        segment
            .files()
            .iter()
            .any(|f| f.name.starts_with("segments_"))
    );
    assert!(
        header
            .block_containing(block.offset, block.length + 1)
            .is_none()
    );
}

/// The whole walk, as a reader does it: descriptor, then commit, then files.
#[test]
fn a_reader_walks_from_the_index_name_to_its_files() {
    let descriptor_blob = blob(&descriptor_key(BASE, INDEX).unwrap());
    let index = DescriptorRecord::parse(Register::parse(&descriptor_blob).unwrap().value)
        .unwrap()
        .into_index()
        .unwrap();
    let shard = index.shard(0).unwrap();
    let manifest_blob = blob(&shard.manifest_key(BASE).unwrap());
    let manifest = CommitManifest::parse(Register::parse(&manifest_blob).unwrap().value).unwrap();
    let extents = shard.resolve(BASE, &manifest).unwrap();
    let keys: std::collections::BTreeSet<&str> = extents.iter().map(|e| e.key.as_str()).collect();
    assert_eq!(
        keys,
        [pack_key().as_str(), segment_blob_key().as_str()]
            .into_iter()
            .collect()
    );
    assert!(matches!(
        index.shard(1),
        Err(oss_format::FormatError::NotFollowable { .. })
    ));
}

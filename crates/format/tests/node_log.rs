//! Write-log records and node-log blobs read and written as the Java's
//! `WalRecord` and `NodeLogBlob` do (docs/specs/node-log-format/SPEC.md,
//! AC-1 and AC-2).

// ARITH: offsets and line numbers here are fixture sizes; an overflow
// panics the test in a debug build, which fails it.
#![allow(clippy::disallowed_methods, clippy::arithmetic_side_effects)]

use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::PathBuf;

use oss_format::node_log_format::{decode, encode, encode_fence, header_end};
use oss_format::{
    EncodedWalRecord, FormatError, NodeLogEntry, NodeLogHeader, OrdinalRange, ShardBatch, ShardKey,
    WalRecord, node_log_key, slice_records,
};

fn fixture(name: &str) -> Vec<String> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/node-log")
        .join(name);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("fixture {}: {e}", path.display()))
        .lines()
        .map(str::to_owned)
        .collect()
}

fn unhex(text: &str) -> Vec<u8> {
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(text.get(i..i + 2).unwrap(), 16).unwrap())
        .collect()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

fn written(record: &WalRecord<'_>) -> String {
    String::from_utf8(record.encode().unwrap().bytes().to_vec()).unwrap()
}

#[test]
fn every_record_reads_and_writes_as_the_javas() {
    let cases = fixture("records.txt");
    let parsed = fixture("records-parsed.txt");
    assert_eq!(parsed.len(), cases.len());
    let mut refused = 0;
    for (case, java) in cases.iter().zip(&parsed) {
        let rust = WalRecord::parse(case.as_bytes());
        if java == "refused" {
            refused += 1;
            assert!(
                matches!(rust, Err(FormatError::Corrupt { .. })),
                "{case}: {rust:?}"
            );
            continue;
        }
        let rust = rust.unwrap_or_else(|e| panic!("{case}: {e}"));
        assert_eq!(written(&rust), *java, "{case}");
        // What the Java wrote reads back to the same bytes; a partial sequence
        // identity, which is not written, is gone.
        assert_eq!(written(&WalRecord::parse(java.as_bytes()).unwrap()), *java);
        assert_eq!(rust.encoded_len(), java.len());
    }
    assert_eq!(refused, 7);
}

#[test]
fn what_the_java_coerces_or_refuses_is_refused() {
    let cases = fixture("records-strict.txt");
    let java = fixture("records-strict-parsed.txt");
    assert_eq!(cases.len(), java.len());
    // The Java takes ten of these by coercing; the Rust refuses all.
    assert_eq!(java.iter().filter(|j| *j != "refused").count(), 10);
    for case in &cases {
        let rust = WalRecord::parse(case.as_bytes());
        assert!(
            matches!(rust, Err(FormatError::Corrupt { .. })),
            "{case}: {rust:?}"
        );
    }
}

#[test]
fn a_parsed_record_says_what_the_javas_accessors_say() {
    let record = WalRecord::parse(
        br#"{"id":"6","source":"{\"x\":true}","seq_no":12,"primary_term":3,"version":2,"routing":"r"}"#,
    )
    .unwrap();
    assert_eq!(record.id(), "6");
    assert_eq!(record.source(), r#"{"x":true}"#);
    assert!(!record.is_deletion());
    assert_eq!(
        (record.seq_no(), record.primary_term(), record.version()),
        (12, 3, 2)
    );
    assert_eq!(record.routing(), Some("r"));
    assert_eq!(record.placed_by(), "r");
    assert!(record.has_sequence_identity());
    let deleted = WalRecord::parse(br#"{"id":"4","source":"","deleted":true}"#).unwrap();
    assert!(deleted.is_deletion());
    assert!(!deleted.has_sequence_identity());
    assert_eq!(
        (deleted.seq_no(), deleted.primary_term(), deleted.version()),
        (-2, 0, -3)
    );
}

/// The records of records.txt the Java accepted, by line number.
fn accepted() -> HashMap<usize, EncodedWalRecord> {
    fixture("records.txt")
        .iter()
        .enumerate()
        .filter_map(|(i, line)| {
            WalRecord::parse(line.as_bytes())
                .ok()
                .map(|r| (i + 1, r.into_owned().encode().unwrap()))
        })
        .collect()
}

fn key(uuid: &str, name: &str, shard: &str) -> ShardKey {
    ShardKey::new(
        uuid.to_owned(),
        shard.parse().unwrap(),
        (name != "-").then(|| name.to_owned()),
    )
}

fn name_of(key: &ShardKey) -> String {
    key.index_name.as_deref().unwrap_or("-").to_owned()
}

/// The Java recorder's `dump`, line for line.
fn dump(entry: &NodeLogEntry<'_>) -> String {
    let h = match entry {
        NodeLogEntry::Fence(f) => return format!("fence by={} term={}", f.fenced_by, f.term),
        NodeLogEntry::Records(h) => h,
    };
    let slices: Vec<String> = h
        .slices
        .iter()
        .map(|s| {
            format!(
                "{}/{}/{}/{}/{}/{}/{}/{}",
                s.shard.index_uuid,
                name_of(&s.shard),
                s.shard.shard,
                s.term,
                s.offset,
                s.length,
                s.records,
                s.max_seq_no
            )
        })
        .collect();
    let index: Vec<String> = h
        .index
        .iter()
        .map(|(k, runs)| {
            let runs: Vec<String> = runs
                .iter()
                .map(|r| format!("{}-{}", r.from, r.to))
                .collect();
            format!(
                "{}/{}/{}={}",
                k.index_uuid,
                name_of(k),
                k.shard,
                runs.join(";")
            )
        })
        .collect();
    format!(
        "records node={} epoch={} ordinal={} trim={} body={} slices=[{}] index=[{}]",
        h.node_id,
        h.epoch,
        h.ordinal,
        h.trim_point,
        h.body_offset,
        slices.join(","),
        index.join(",")
    )
}

struct Case {
    blob: Vec<String>,
    prior: Vec<(ShardKey, Vec<OrdinalRange>)>,
    batches: Vec<(ShardKey, i64, Vec<usize>)>,
}

enum Built {
    Blob(Result<Vec<u8>, FormatError>, Case),
    Fence(Vec<u8>),
}

fn build_cases() -> Vec<Built> {
    let records = accepted();
    let mut out = Vec::new();
    let mut case: Option<Case> = None;
    for line in fixture("blob-cases.txt") {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let t: Vec<&str> = line.split(' ').collect();
        match t[0] {
            "blob" => {
                case = Some(Case {
                    blob: t.iter().map(|s| (*s).to_owned()).collect(),
                    prior: Vec::new(),
                    batches: Vec::new(),
                });
            }
            "prior" => {
                let runs = t[4]
                    .split(',')
                    .map(|r| {
                        let (a, b) = r.split_once('-').unwrap();
                        OrdinalRange {
                            from: a.parse().unwrap(),
                            to: b.parse().unwrap(),
                        }
                    })
                    .collect();
                case.as_mut()
                    .unwrap()
                    .prior
                    .push((key(t[1], t[2], t[3]), runs));
            }
            "batch" => {
                let lines = if t[5] == "none" {
                    Vec::new()
                } else {
                    t[5].split(',').map(|n| n.parse().unwrap()).collect()
                };
                case.as_mut().unwrap().batches.push((
                    key(t[1], t[2], t[3]),
                    t[4].parse().unwrap(),
                    lines,
                ));
            }
            "end" => {
                let c = case.take().unwrap();
                let held: Vec<Vec<EncodedWalRecord>> = c
                    .batches
                    .iter()
                    .map(|(_, _, lines)| lines.iter().map(|n| records[n].clone()).collect())
                    .collect();
                let batches: Vec<ShardBatch<'_>> = c
                    .batches
                    .iter()
                    .zip(&held)
                    .map(|((shard, term, _), records)| ShardBatch {
                        shard,
                        term: *term,
                        records,
                    })
                    .collect();
                let built = encode(
                    &c.blob[1],
                    c.blob[2].parse().unwrap(),
                    c.blob[3].parse().unwrap(),
                    c.blob[4].parse().unwrap(),
                    &batches,
                    &c.prior,
                )
                .map(|e| {
                    // The returned header is what decoding the bytes reads.
                    assert_eq!(
                        decode(&e.bytes).unwrap(),
                        NodeLogEntry::Records(e.header.clone())
                    );
                    e.bytes
                });
                out.push(Built::Blob(built, c));
            }
            "fence" => out.push(Built::Fence(
                encode_fence(t[1], t[2].parse().unwrap()).unwrap(),
            )),
            other => panic!("unknown line {other}"),
        }
    }
    out
}

#[test]
fn every_blob_encodes_as_the_javas() {
    let java = fixture("blobs.txt");
    let built = build_cases();
    assert_eq!(built.len(), java.len());
    let mut refused = 0;
    for (built, java) in built.iter().zip(&java) {
        match built {
            Built::Blob(Err(e), c) => {
                refused += 1;
                assert_eq!(java, "refused", "{:?}: {e}", c.blob);
                assert!(matches!(e, FormatError::Corrupt { .. }), "{e:?}");
            }
            Built::Blob(Ok(bytes), c) => assert_eq!(hex(bytes), *java, "{:?}", c.blob),
            Built::Fence(bytes) => assert_eq!(hex(bytes), *java),
        }
    }
    assert_eq!(refused, 3);
}

#[test]
fn every_java_blob_decodes_to_what_the_java_read() {
    let blobs = fixture("blobs.txt");
    let decoded = fixture("decoded.txt");
    let records = accepted();
    let built = build_cases();
    assert_eq!(blobs.len(), decoded.len());
    for ((java, read), built) in blobs.iter().zip(&decoded).zip(&built) {
        if java == "refused" {
            continue;
        }
        let bytes = unhex(java);
        let entry = decode(&bytes).unwrap_or_else(|e| panic!("{read}: {e}"));
        assert_eq!(dump(&entry), *read);
        let (NodeLogEntry::Records(h), Built::Blob(_, case)) = (&entry, built) else {
            continue;
        };
        assert_eq!(
            header_end(&bytes).unwrap(),
            Some(usize::try_from(h.body_offset).unwrap())
        );
        assert_eq!(h.body_offset + h.body_length(), bytes.len() as u64);
        // Every slice reads back to the records its batch held.
        let written: Vec<&(ShardKey, i64, Vec<usize>)> =
            case.batches.iter().filter(|b| !b.2.is_empty()).collect();
        assert_eq!(h.slices.len(), written.len());
        for (slice, (shard, term, lines)) in h.slices.iter().zip(written) {
            assert_eq!(slice.shard, *shard);
            assert_eq!(slice.term, *term);
            let start = usize::try_from(h.body_offset + slice.offset).unwrap();
            let body = &bytes[start..start + slice.length as usize];
            let back: Vec<WalRecord<'_>> = slice_records(body).map(Result::unwrap).collect();
            let want: Vec<WalRecord<'_>> =
                lines.iter().map(|n| records[n].record().unwrap()).collect();
            assert_eq!(back, want);
            assert_eq!(h.slices_of(shard).count(), 1);
        }
    }
}

fn first_records_blob() -> Vec<u8> {
    let blobs = fixture("blobs.txt");
    let decoded = fixture("decoded.txt");
    // The case with three slices and a running index of three shards.
    let at = decoded
        .iter()
        .position(|d| d.contains("node=node-7"))
        .unwrap();
    unhex(&blobs[at])
}

#[test]
fn a_header_reads_ordinals_of_a_shard_and_its_slices() {
    let bytes = first_records_blob();
    let NodeLogEntry::Records(h) = decode(&bytes).unwrap() else {
        panic!("not records");
    };
    let ua = ShardKey::new("uA", 0, None::<&str>);
    assert_eq!(
        h.ordinals_of(&ua),
        [
            OrdinalRange { from: 2, to: 3 },
            OrdinalRange { from: 5, to: 5 }
        ]
    );
    assert_eq!(h.ordinals_of(&ShardKey::new("uC", 2, None::<&str>)), []);
    assert_eq!(h.slices_of(&ua).count(), 1);
    assert_eq!(
        h.slices_of(&ShardKey::new("uD", 0, None::<&str>)).count(),
        0
    );
}

#[test]
fn an_empty_window_and_an_index_only_window_encode() {
    let prior = [(
        ShardKey::new("u", 3, Some("i")),
        vec![OrdinalRange { from: 1, to: 2 }],
    )];
    let blob = encode("n", 0, 4, 1, &[], &prior).unwrap();
    assert_eq!(blob.header.slices, []);
    assert_eq!(blob.header.index.len(), 1);
    assert_eq!(blob.header.body_length(), 0);
    assert_eq!(blob.bytes.len() as u64, blob.header.body_offset);
    let NodeLogEntry::Records(h): NodeLogEntry<'_> = decode(&blob.bytes).unwrap() else {
        panic!()
    };
    let _: &NodeLogHeader<'_> = &h;
    assert_eq!(
        h.ordinals_of(&prior[0].0),
        [OrdinalRange { from: 1, to: 2 }]
    );
}

#[test]
fn slice_records_skips_blank_lines_and_stops_at_a_bad_one() {
    let body = b"\n{\"id\":\"1\",\"source\":\"\"}\n\n{\"id\":\"2\",\"source\":\"\"}\n";
    let ids: Vec<String> = slice_records(body)
        .map(|r| r.unwrap().id().to_owned())
        .collect();
    assert_eq!(ids, ["1", "2"]);
    assert_eq!(slice_records(b"").count(), 0);
    let mut bad = slice_records(b"{\"id\":\"1\",\"source\":\"\"}\nnot json");
    assert!(bad.next().unwrap().is_ok());
    assert!(bad.next().unwrap().is_err());
    assert!(bad.next().is_none());
}

#[test]
fn a_key_names_its_slot() {
    assert_eq!(
        node_log_key("base/", "n1", 3, 12).unwrap(),
        "base/nodelog/n1/e=00000000000000000003/00000000000000000012"
    );
    assert_eq!(
        node_log_key("", "n1", 0, 0).unwrap(),
        "nodelog/n1/e=00000000000000000000/00000000000000000000"
    );
    assert!(node_log_key("base", "n1", 0, 0).is_err());
    assert!(node_log_key("", "a/b", 0, 0).is_err());
}

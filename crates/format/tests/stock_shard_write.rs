//! docs/specs/stock-snapshot-export AC-4 (the shard snapshot and shard
//! generation writers): the shard snapshot lists the files by name, length
//! and checksum the recorded one does, the checksum is the file's own footer
//! CRC, and the generation blob reads back byte for byte and extends with a
//! snapshot.

use oss_format::{
    FormatError, StockEnvelopeLimits, StockNewFile, StockShardGeneration, StockShardSnapshot,
    stock_blob_to_json, stock_checksum_string, stock_shard_snapshot_json,
};

const REPO: &str = "../../fixtures/snapshots/stock/repo/indices/M3IuI3tpQ7mcpIwUIA1kfQ/0/";

#[allow(clippy::disallowed_methods)] // ARITH: a test reads a recorded fixture.
fn bytes(file: &str) -> Vec<u8> {
    let path = format!("{}/{REPO}{file}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

fn json(file: &str, codec: &str) -> String {
    let mut out = Vec::new();
    stock_blob_to_json(
        &bytes(file),
        codec,
        &StockEnvelopeLimits::default(),
        &mut out,
    )
    .expect("decodes");
    String::from_utf8(out).expect("UTF-8")
}

fn footer_crc(blob: &[u8]) -> u32 {
    let tail = blob.get(blob.len().saturating_sub(8)..).unwrap();
    u32::try_from(u64::from_be_bytes(tail.try_into().unwrap())).unwrap()
}

fn text(f: impl FnOnce(&mut Vec<u8>) -> Result<(), FormatError>) -> String {
    let mut out = Vec::new();
    f(&mut out).expect("written");
    String::from_utf8(out).expect("UTF-8")
}

const SNAP: &str = "snap-q6tlFJQcRUS97oE_eOaE7Q.dat";
const GEN: &str = "index-wnHW9wrYQAGcMm1YPawcow";

#[test]
fn the_checksum_is_the_footer_crc_in_base_36() {
    // The two recorded blobs, whose entries the recorded snapshot names.
    assert_eq!(
        stock_checksum_string(footer_crc(&bytes("__L7JGf21KRju_ZczIDWC4kA"))),
        "49j5mw"
    );
    assert_eq!(
        stock_checksum_string(footer_crc(&bytes("__WdrkK07SSX2VoQ3EFgh6bQ"))),
        "hqxs97"
    );
    assert_eq!(stock_checksum_string(0), "0");
    assert_eq!(stock_checksum_string(u32::MAX), "1z141z3");
}

fn recorded_files() -> Vec<StockNewFile<'static>> {
    let read = StockShardSnapshot::parse(json(SNAP, "snapshot").as_bytes()).unwrap();
    // The recorded snapshot inlines `_0.si` and `segments_3`; the export
    // writes them as blobs like the rest, so only the first-class fields
    // are taken.
    read.files
        .iter()
        .map(|f| StockNewFile {
            // A virtual file's blob name starts `v__`; the export's blobs are `__<id>`.
            name: Box::leak(f.name.replacen("v__", "__", 1).into_boxed_str()),
            physical_name: Box::leak(f.physical_name.clone().into_boxed_str()),
            length: f.length,
            checksum: Box::leak(f.checksum.clone().into_boxed_str()),
            written_by: "10.5.1",
        })
        .collect()
}

#[test]
fn the_shard_snapshot_lists_the_recorded_files_and_their_sums() {
    let files = recorded_files();
    let got = text(|o| stock_shard_snapshot_json("snap-1", 3, 1_795_652_423_613, 0, &files, o));
    assert!(
        got.starts_with(r#"{"name":"snap-1","index_version":3,"start_time":1795652423613,"time":0,"number_of_files":4,"total_size":6137,"files":["#),
        "{got}"
    );
    let back = StockShardSnapshot::parse(got.as_bytes()).unwrap();
    let want = StockShardSnapshot::parse(json(SNAP, "snapshot").as_bytes()).unwrap();
    assert_eq!(back.name, want.name);
    assert_eq!(back.files.len(), want.files.len());
    for (b, w) in back.files.iter().zip(&want.files) {
        assert_eq!(b.name, w.name.replacen("v__", "__", 1));
        assert_eq!(
            (&b.physical_name, b.length, &b.checksum, &b.written_by),
            (&w.physical_name, w.length, &w.checksum, &w.written_by)
        );
        assert_eq!(
            b.part_size,
            Some(i64::MAX as u64),
            "an unsplit file, as stock writes it"
        );
        assert!(!b.is_virtual());
    }
}

#[test]
fn the_recorded_shard_generation_is_written_back_byte_for_byte() {
    let want = json(GEN, "snapshots");
    let g = StockShardGeneration::parse(want.as_bytes()).unwrap();
    assert_eq!(text(|o| g.to_json(o)), want);
}

#[test]
fn a_snapshot_joins_the_generation_with_its_files() {
    let g = StockShardGeneration::parse(json(GEN, "snapshots").as_bytes()).unwrap();
    let new = [StockNewFile {
        name: "__NewBlob0000000000000A",
        physical_name: "_1.cfs",
        length: 10,
        checksum: "abc",
        written_by: "10.5.1",
    }];
    let next = g.with_snapshot("snap-2", &new).unwrap();
    let back = StockShardGeneration::parse(text(|o| next.to_json(o)).as_bytes()).unwrap();
    assert_eq!(back.snapshots(), ["snap-1", "snap-2"]);
    assert_eq!(
        back.files_of("snap-2").unwrap(),
        ["__NewBlob0000000000000A"]
    );
    assert_eq!(back.files_of("snap-1").unwrap().len(), 4);
    assert_eq!(back.file_count(), 5);
    // The earlier snapshot's entry, `shard_state_id` included, is kept as read.
    assert!(
        text(|o| next.to_json(o))
            .contains(r#""shard_state_id":"yJ6_SVGMTMaPMgZ6jCsXEg-P-Nlz0JRS8KkjQukqz-90Q-19""#)
    );
}

#[test]
fn an_empty_shard_starts_a_generation_of_its_first_snapshot() {
    let g = StockShardGeneration::default();
    let f = [StockNewFile {
        name: "__A",
        physical_name: "_0.cfs",
        length: 1,
        checksum: "1",
        written_by: "10.5.1",
    }];
    let next = g.with_snapshot("s", &f).unwrap();
    let back = StockShardGeneration::parse(text(|o| next.to_json(o)).as_bytes()).unwrap();
    assert_eq!(back.snapshots(), ["s"]);
}

#[test]
fn a_repeated_snapshot_or_file_name_or_an_unsafe_one_is_refused() {
    let g = StockShardGeneration::parse(json(GEN, "snapshots").as_bytes()).unwrap();
    let file = |name| StockNewFile {
        name,
        physical_name: "_1.cfs",
        length: 1,
        checksum: "1",
        written_by: "10.5.1",
    };
    assert!(
        g.with_snapshot("snap-1", &[file("__x")]).is_err(),
        "a held snapshot name"
    );
    assert!(g.with_snapshot("", &[file("__x")]).is_err(), "no name");
    assert!(
        g.with_snapshot("s", &[file("__L7JGf21KRju_ZczIDWC4kA")])
            .is_err(),
        "a held blob"
    );
    assert!(
        g.with_snapshot("s", &[file("__x"), file("__x")]).is_err(),
        "a blob twice"
    );
    assert!(
        g.with_snapshot("s", &[file("../x")]).is_err(),
        "an unsafe blob name"
    );
    assert!(g.with_snapshot("s", &[]).is_err(), "no files");
}

#[test]
fn a_bad_file_is_refused_and_the_buffer_keeps_its_start() {
    let ok = StockNewFile {
        name: "__A",
        physical_name: "_0.cfs",
        length: 1,
        checksum: "1",
        written_by: "10.5.1",
    };
    let bad: Vec<(&str, StockNewFile<'_>)> = vec![
        ("unsafe blob", StockNewFile { name: "a/b", ..ok }),
        ("a virtual file's name", StockNewFile { name: "v__A", ..ok }),
        (
            "empty physical name",
            StockNewFile {
                physical_name: "",
                ..ok
            },
        ),
        ("empty checksum", StockNewFile { checksum: "", ..ok }),
        (
            "non-base-36 checksum",
            StockNewFile {
                checksum: "\"x",
                ..ok
            },
        ),
    ];
    for (why, f) in bad {
        let mut out = b"keep".to_vec();
        assert!(
            stock_shard_snapshot_json("s", 1, 0, 0, &[f], &mut out).is_err(),
            "{why}"
        );
        assert_eq!(out, b"keep", "{why}");
    }
    let mut out = b"keep".to_vec();
    assert!(
        stock_shard_snapshot_json("s", 1, 0, 0, &[], &mut out).is_err(),
        "no files"
    );
    assert_eq!(out, b"keep");
}

#[test]
fn the_sums_hold_across_many_files() {
    let mk = |n: usize| (0..n).map(|i| format!("__f{i}")).collect::<Vec<_>>();
    let names = mk(3);
    let files: Vec<StockNewFile<'_>> = names
        .iter()
        .enumerate()
        .map(|(i, n)| StockNewFile {
            name: n,
            physical_name: "_0.cfs",
            length: 10 * (i as u64 + 1),
            checksum: "z",
            written_by: "10.5.1",
        })
        .collect();
    let got = text(|o| stock_shard_snapshot_json("s", 1, 0, 0, &files, o));
    assert!(
        got.contains(r#""number_of_files":3,"total_size":60"#),
        "{got}"
    );
}

/// `OSS_WRITE_STOCK=dir` leaves the blobs for `CheckStockWrite.java`.
#[test]
#[allow(clippy::disallowed_methods)] // ARITH: a test writes a scratch directory.
fn the_shard_blobs_are_left_for_the_java_check() {
    let Ok(dir) = std::env::var("OSS_WRITE_STOCK") else {
        return;
    };
    std::fs::create_dir_all(&dir).unwrap();
    let put = |name: &str, codec: &str, json: &str| {
        let mut blob = Vec::new();
        oss_format::stock_blob_from_json(
            json.as_bytes(),
            codec,
            &StockEnvelopeLimits::default(),
            &mut blob,
        )
        .unwrap();
        std::fs::write(format!("{dir}/{name}"), blob).unwrap();
    };
    let files = recorded_files();
    put(
        "shard-snapshot.dat",
        "snapshot",
        &text(|o| stock_shard_snapshot_json("snap-2", 3, 1, 2, &files, o)),
    );
    let g = StockShardGeneration::parse(json(GEN, "snapshots").as_bytes()).unwrap();
    let new = [StockNewFile {
        name: "__NewBlob0000000000000A",
        physical_name: "_1.cfs",
        length: 10,
        checksum: "abc",
        written_by: "10.5.1",
    }];
    let next = g.with_snapshot("snap-2", &new).unwrap();
    put(
        "shard-generation.dat",
        "snapshots",
        &text(|o| next.to_json(o)),
    );
}

#[test]
fn the_file_count_bound_holds_at_the_limit_and_one_past() {
    let names: Vec<String> = (0..10_001).map(|i| format!("__f{i}")).collect();
    let files = |n: usize| -> Vec<StockNewFile<'_>> {
        names
            .iter()
            .take(n)
            .map(|name| StockNewFile {
                name,
                physical_name: "_0.cfs",
                length: 1,
                checksum: "z",
                written_by: "10.5.1",
            })
            .collect()
    };
    let ok = |n: usize| stock_shard_snapshot_json("s", 1, 0, 0, &files(n), &mut Vec::new()).is_ok();
    assert!(ok(10_000));
    assert!(!ok(10_001));
}

const ONE: &str = r#"{"files":[{"name":"__a","physical_name":"_0.cfs","length":5,"checksum":"a b","part_size":5,"written_by":"10.5.1"}],"snapshots":{"s":{"files":["__a"]}}}"#;

#[test]
fn a_part_size_and_a_free_form_checksum_are_kept_as_read() {
    let g = StockShardGeneration::parse(ONE.as_bytes()).unwrap();
    assert_eq!(text(|o| g.to_json(o)), ONE);
}

#[test]
fn a_generation_with_an_unsafe_blob_name_or_too_many_entries_is_refused() {
    let bad_file = ONE.replacen(r#""name":"__a""#, r#""name":"../a""#, 1);
    assert!(StockShardGeneration::parse(bad_file.as_bytes()).is_err());
    let bad_ref = ONE.replace(r#"["__a"]"#, r#"["../a"]"#);
    assert!(StockShardGeneration::parse(bad_ref.as_bytes()).is_err());
    // One file, one snapshot and one reference: three entries.
    assert!(StockShardGeneration::parse_with(ONE.as_bytes(), 3).is_ok());
    assert!(StockShardGeneration::parse_with(ONE.as_bytes(), 2).is_err());
}

#[test]
fn the_checksum_digits_run_to_the_top_of_the_range() {
    // Powers of 36 and their neighbours, worked by hand.
    assert_eq!(stock_checksum_string(35), "z");
    assert_eq!(stock_checksum_string(36), "10");
    assert_eq!(stock_checksum_string(1295), "zz");
    assert_eq!(stock_checksum_string(1296), "100");
}

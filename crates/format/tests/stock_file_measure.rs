//! docs/specs/stock-snapshot-export AC-5 (the measure of a copied file) and
//! AC-4 (`written_by`): a file whose checksum is not listed yet is measured
//! as its bytes stream past, its footer CRC and length checked and the CRC
//! returned; the Lucene version a segment's `.si` or a `segments_N` records
//! is read from the file's head.

use oss_format::{
    FormatError, StockEnvelopeLimits, StockFileCheck, StockShardSnapshot, lucene_version_of,
    stock_blob_to_json, stock_checksum_string,
};

const REPO: &str = "../../fixtures/snapshots/stock/repo/indices/M3IuI3tpQ7mcpIwUIA1kfQ/0/";

#[allow(clippy::disallowed_methods)] // ARITH: a test reads a recorded fixture.
fn blob(file: &str) -> Vec<u8> {
    let path = format!("{}/{REPO}{file}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

/// The recorded `_0.si` and `segments_3`, which the stock snapshot inlines.
fn inline(physical: &str) -> (Vec<u8>, String) {
    let mut out = Vec::new();
    stock_blob_to_json(
        &blob("snap-q6tlFJQcRUS97oE_eOaE7Q.dat"),
        "snapshot",
        &StockEnvelopeLimits::default(),
        &mut out,
    )
    .unwrap();
    let s = StockShardSnapshot::parse(&out).unwrap();
    let f = s
        .files
        .iter()
        .find(|f| f.physical_name == physical)
        .unwrap();
    (f.inline.clone().unwrap(), f.checksum.clone())
}

fn measure(bytes: &[u8], chunk: usize) -> Result<u32, FormatError> {
    let mut check = StockFileCheck::measured(bytes.len() as u64, 256)?;
    for part in bytes.chunks(chunk) {
        check.update(part)?;
    }
    check.finish_crc()
}

#[test]
fn the_recorded_files_measure_to_their_recorded_checksums_in_any_chunking() {
    let (si, si_sum) = inline("_0.si");
    let (seg, seg_sum) = inline("segments_3");
    for chunk in [1usize, 7, 16, 17, 100, 4096] {
        assert_eq!(
            stock_checksum_string(measure(&si, chunk).unwrap()),
            si_sum,
            "{chunk}"
        );
        assert_eq!(
            stock_checksum_string(measure(&seg, chunk).unwrap()),
            seg_sum,
            "{chunk}"
        );
    }
    assert_eq!(
        stock_checksum_string(measure(&blob("__L7JGf21KRju_ZczIDWC4kA"), 1000).unwrap()),
        "49j5mw"
    );
    assert_eq!(
        stock_checksum_string(measure(&blob("__WdrkK07SSX2VoQ3EFgh6bQ"), 1000).unwrap()),
        "hqxs97"
    );
}

#[test]
fn a_flipped_byte_a_short_or_long_file_and_a_bad_footer_are_refused() {
    let (si, _) = inline("_0.si");
    let mut flipped = si.clone();
    flipped[40] ^= 1;
    assert!(measure(&flipped, 64).is_err(), "a flipped body byte");
    let mut bad_crc = si.clone();
    let n = bad_crc.len();
    bad_crc[n - 1] ^= 1;
    assert!(measure(&bad_crc, 64).is_err(), "a flipped footer byte");
    let mut bad_magic = si.clone();
    let n = bad_magic.len();
    bad_magic[n - 16] ^= 1;
    assert!(
        measure(&bad_magic, 64).is_err(),
        "a footer without the magic"
    );
    // The length is what the manifest said: short and long both fail.
    let mut short = StockFileCheck::measured(si.len() as u64, 0).unwrap();
    short.update(&si[..si.len() - 1]).unwrap();
    assert!(short.finish_crc().is_err());
    let mut long = StockFileCheck::measured(si.len() as u64, 0).unwrap();
    assert!(long.update(&[si.as_slice(), &[0]].concat()).is_err());
    assert!(
        StockFileCheck::measured(15, 0).is_err(),
        "shorter than a footer"
    );
}

#[test]
fn the_head_keeps_the_first_bytes_only_up_to_its_bound() {
    let (si, _) = inline("_0.si");
    let mut check = StockFileCheck::measured(si.len() as u64, 50).unwrap();
    for part in si.chunks(7) {
        check.update(part).unwrap();
    }
    assert_eq!(check.head(), &si[..50]);
    let mut none = StockFileCheck::measured(si.len() as u64, 0).unwrap();
    none.update(&si).unwrap();
    assert!(none.head().is_empty());
}

#[test]
fn a_listed_check_still_compares_its_listed_checksum() {
    let (si, sum) = inline("_0.si");
    let mut ok = StockFileCheck::new(si.len() as u64, &sum).unwrap();
    ok.update(&si).unwrap();
    assert!(ok.finish().is_ok());
    let mut wrong = StockFileCheck::new(si.len() as u64, "1").unwrap();
    wrong.update(&si).unwrap();
    assert!(wrong.finish().is_err());
}

#[test]
fn the_lucene_version_is_read_from_a_segment_info_and_a_segments_file() {
    let (si, _) = inline("_0.si");
    let (seg, _) = inline("segments_3");
    assert_eq!(lucene_version_of(&si, true).unwrap(), "10.5.1");
    assert_eq!(lucene_version_of(&seg, false).unwrap(), "10.5.1");
    // Only the head is needed.
    assert_eq!(lucene_version_of(&si[..80], true).unwrap(), "10.5.1");
}

#[test]
fn a_head_that_is_not_a_lucene_header_or_is_cut_short_is_refused() {
    let (si, _) = inline("_0.si");
    assert!(
        lucene_version_of(&si[..30], true).is_err(),
        "cut inside the header"
    );
    assert!(lucene_version_of(&[0u8; 80], true).is_err(), "no magic");
    assert!(lucene_version_of(&[], false).is_err());
    // A major version of 0 or one past any Lucene is not a version.
    let at = 4 + 1 + usize::from(si[4]) + 4 + 16 + 1;
    let mut zero = si.clone();
    zero[at] = 0;
    assert!(lucene_version_of(&zero, true).is_err());
    let mut huge = si.clone();
    huge[at] = 200;
    assert!(lucene_version_of(&huge, true).is_err());
}

/// A synthetic Lucene head: a codec name and suffix of the given lengths,
/// then a version as a segment info or a segments file writes it.
fn synthetic(codec: usize, suffix: usize, v: [u32; 3], si: bool) -> Vec<u8> {
    let mut h = vec![0x3f, 0xd7, 0x6c, 0x17, u8::try_from(codec).unwrap()];
    h.extend(std::iter::repeat_n(b'c', codec));
    h.extend_from_slice(&[0, 0, 0, 0]);
    h.extend_from_slice(&[7u8; 16]);
    h.push(u8::try_from(suffix).unwrap());
    h.extend(std::iter::repeat_n(b's', suffix));
    for n in v {
        if si {
            h.extend_from_slice(&n.to_le_bytes());
        } else {
            let mut n = n;
            while n >= 0x80 {
                h.push(u8::try_from(n & 0x7f).unwrap() | 0x80);
                n >>= 7;
            }
            h.push(u8::try_from(n).unwrap());
        }
    }
    h
}

#[test]
fn the_bounds_of_a_head_and_a_version_hold_exactly() {
    for si in [true, false] {
        let ok = |c, s, v| lucene_version_of(&synthetic(c, s, v, si), si);
        assert_eq!(ok(127, 127, [99, 9900, 9999]).unwrap(), "99.9900.9999");
        assert_eq!(ok(1, 0, [1, 0, 0]).unwrap(), "1.0.0");
        assert!(ok(128, 0, [10, 5, 1]).is_err(), "codec past its bound");
        assert!(ok(0, 0, [10, 5, 1]).is_err(), "empty codec");
        assert!(ok(1, 128, [10, 5, 1]).is_err(), "suffix past its bound");
        assert!(ok(1, 0, [100, 0, 0]).is_err(), "major past its bound");
        assert!(ok(1, 0, [0, 0, 0]).is_err(), "major 0");
        assert!(ok(1, 0, [10, 9901, 0]).is_err(), "minor past its bound");
        assert!(ok(1, 0, [10, 0, 10_000]).is_err(), "bugfix past its bound");
    }
}

#[test]
fn a_file_of_exactly_a_footer_is_measured_and_a_shorter_one_is_refused() {
    assert!(StockFileCheck::measured(16, 0).is_ok());
    assert!(StockFileCheck::measured(15, 0).is_err());
}

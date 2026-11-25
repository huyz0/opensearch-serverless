//! docs/specs/stock-snapshot-restore AC-2: a blob in the stock envelope is
//! decoded only after its footer checksum verifies, a compressed body only up
//! to its limit, and a wrong codec, a short blob, a flipped byte or a bomb is
//! refused. The blobs are written by Lucene's `CodecUtil` and the JDK's
//! `Deflater` (`fixtures/snapshots/stock-envelope/GenEnvelope.java`).

use oss_format::{
    FormatError, SmileLimits, StockEnvelopeLimits, smile_to_json, stock_blob_to_json,
    stock_verified_body,
};

#[allow(clippy::disallowed_methods)] // ARITH: a test reads a recorded fixture.
fn fixture(name: &str) -> Vec<u8> {
    let path = format!(
        "{}/../../fixtures/snapshots/stock-envelope/{name}",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

fn json(blob: &[u8], codec: &str) -> Result<String, FormatError> {
    let mut out = Vec::new();
    stock_blob_to_json(blob, codec, &StockEnvelopeLimits::default(), &mut out)?;
    Ok(String::from_utf8(out).expect("JSON is UTF-8"))
}

fn expected() -> String {
    let mut out = Vec::new();
    smile_to_json(&fixture("body.smile"), &SmileLimits::default(), &mut out).expect("body");
    String::from_utf8(out).expect("UTF-8")
}

/// A blob in the envelope: the header, the body, the footer with its CRC.
fn wrap(codec: &str, version: u32, body: &[u8]) -> Vec<u8> {
    let mut v = vec![0x3F, 0xD7, 0x6C, 0x17, codec.len() as u8];
    v.extend(codec.as_bytes());
    v.extend(version.to_be_bytes());
    v.extend(body);
    v.extend([0xC0, 0x28, 0x93, 0xE8, 0, 0, 0, 0]);
    let crc = crc32fast::hash(&v);
    v.extend(u64::from(crc).to_be_bytes());
    v
}

#[test]
fn the_javas_plain_and_deflated_blobs_decode_to_the_same_json() {
    let want = expected();
    assert!(want.contains("\"abc-DEF_123\""), "{want}");
    assert_eq!(
        json(&fixture("snapshot.plain.dat"), "snapshot").unwrap(),
        want
    );
    assert_eq!(
        json(&fixture("snapshot.deflate.dat"), "snapshot").unwrap(),
        want
    );
    assert_eq!(json(&fixture("meta.plain.dat"), "metadata").unwrap(), want);
}

#[test]
fn a_wrong_codec_name_is_refused() {
    let blob = fixture("snapshot.plain.dat");
    assert!(matches!(
        json(&blob, "metadata"),
        Err(FormatError::Corrupt { .. })
    ));
    assert!(matches!(
        json(&blob, "snapshots"),
        Err(FormatError::Corrupt { .. })
    ));
}

#[test]
fn every_truncation_of_a_blob_is_refused() {
    for name in ["snapshot.plain.dat", "snapshot.deflate.dat"] {
        let blob = fixture(name);
        for n in 0..blob.len() {
            assert!(
                json(&blob[..n], "snapshot").is_err(),
                "{name} cut at {n} was accepted"
            );
        }
    }
}

#[test]
fn every_flipped_byte_is_refused() {
    for name in ["snapshot.plain.dat", "snapshot.deflate.dat"] {
        let blob = fixture(name);
        for i in 0..blob.len() {
            let mut bad = blob.clone();
            bad[i] ^= 0x01;
            assert!(
                json(&bad, "snapshot").is_err(),
                "{name} with byte {i} flipped was accepted"
            );
        }
    }
}

#[test]
fn a_version_other_than_one_and_a_foreign_checksum_are_refused() {
    let body = fixture("body.smile");
    assert!(json(&wrap("snapshot", 1, &body), "snapshot").is_ok());
    assert!(matches!(
        json(&wrap("snapshot", 2, &body), "snapshot"),
        Err(FormatError::Unsupported { version: 2, .. })
    ));
    let mut other = wrap("snapshot", 1, &body);
    let at = other.len() - 12;
    other[at + 3] = 1; // an algorithm id other than 0, with the CRC recomputed
    let crc = crc32fast::hash(&other[..other.len() - 8]);
    let n = other.len();
    other[n - 8..].copy_from_slice(&u64::from(crc).to_be_bytes());
    assert!(matches!(
        json(&other, "snapshot"),
        Err(FormatError::Corrupt { .. })
    ));
}

#[test]
fn a_crc_with_bits_past_32_is_refused() {
    let body = fixture("body.smile");
    let mut blob = wrap("snapshot", 1, &body);
    let n = blob.len();
    blob[n - 8] = 1;
    assert!(json(&blob, "snapshot").is_err());
}

#[test]
fn a_codec_name_with_a_runaway_length_is_refused() {
    // A name length of 200 (two vint bytes) and one past the cap of 128.
    for len in [128usize, 129] {
        let mut v = vec![0x3F, 0xD7, 0x6C, 0x17];
        v.push(0x80 | (len & 0x7F) as u8);
        v.push((len >> 7) as u8);
        v.extend(vec![b'a'; len]);
        v.extend(1u32.to_be_bytes());
        v.extend([0xC0, 0x28, 0x93, 0xE8, 0, 0, 0, 0]);
        let crc = crc32fast::hash(&v);
        v.extend(u64::from(crc).to_be_bytes());
        let expect = "a".repeat(len);
        let got = stock_verified_body(&v, &expect);
        assert_eq!(got.is_ok(), len == 128, "name length {len}");
    }
    // A vint that never ends.
    let mut v = vec![0x3F, 0xD7, 0x6C, 0x17, 0x80, 0x80, 0x80, 0x80];
    v.extend([0xC0, 0x28, 0x93, 0xE8, 0, 0, 0, 0]);
    let crc = crc32fast::hash(&v);
    v.extend(u64::from(crc).to_be_bytes());
    assert!(stock_verified_body(&v, "x").is_err());
}

#[test]
fn a_deflate_bomb_is_refused_at_the_limit_and_not_before() {
    let raw = vec![b' '; 1 << 20];
    let mut body = b"DFL\0".to_vec();
    body.extend(miniz_oxide::deflate::compress_to_vec(&raw, 9));
    let blob = wrap("snapshot", 1, &body);
    assert!(
        body.len() < 4096,
        "a megabyte of one byte packs small: {}",
        body.len()
    );
    let with = |n: usize| StockEnvelopeLimits {
        decompressed_bytes: n,
        ..StockEnvelopeLimits::default()
    };
    // The decompressed body is spaces, not SMILE: the decode, not the
    // inflate, refuses it when the limit is not hit.
    let mut out = Vec::new();
    let ok = stock_blob_to_json(&blob, "snapshot", &with(1 << 20), &mut out);
    assert!(
        !matches!(ok, Err(FormatError::TooLarge { .. })),
        "{ok:?} at the exact size"
    );
    assert!(matches!(
        stock_blob_to_json(&blob, "snapshot", &with((1 << 20) - 1), &mut out),
        Err(FormatError::TooLarge { .. })
    ));
}

#[test]
fn a_malformed_deflate_body_is_refused() {
    let mut body = b"DFL\0".to_vec();
    body.extend([0xFF, 0xFF, 0xFF, 0xFF]);
    assert!(matches!(
        json(&wrap("snapshot", 1, &body), "snapshot"),
        Err(FormatError::Corrupt { .. })
    ));
}

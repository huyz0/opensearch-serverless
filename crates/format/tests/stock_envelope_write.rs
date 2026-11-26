//! docs/specs/stock-snapshot-export AC-2: a blob written in the stock
//! envelope reads back through the restore's own reader at every body length,
//! under the codec it was written for, and a failed write leaves the buffer
//! as it was.

use oss_format::{
    FormatError, SmileLimits, StockEnvelopeLimits, stock_blob_from_json, stock_blob_to_json,
    stock_verified_body,
};

fn written(json: &str, codec: &str) -> Vec<u8> {
    let mut out = Vec::new();
    stock_blob_from_json(
        json.as_bytes(),
        codec,
        &StockEnvelopeLimits::default(),
        &mut out,
    )
    .expect("written");
    out
}

fn read(blob: &[u8], codec: &str) -> Result<String, FormatError> {
    let mut out = Vec::new();
    stock_blob_to_json(blob, codec, &StockEnvelopeLimits::default(), &mut out)?;
    Ok(String::from_utf8(out).expect("UTF-8"))
}

#[test]
fn a_written_blob_reads_back_at_every_body_length() {
    for n in 0..300usize {
        let json = format!("{{\"k\":\"{}\"}}", "x".repeat(n));
        let blob = written(&json, "snapshot");
        assert_eq!(read(&blob, "snapshot").unwrap(), json, "{n}");
    }
}

#[test]
fn the_header_names_the_codec_at_version_one_and_the_footer_sums_it() {
    let blob = written("{}", "index-metadata");
    assert_eq!(&blob[..4], &[0x3F, 0xD7, 0x6C, 0x17]);
    assert_eq!(blob[4] as usize, "index-metadata".len());
    assert_eq!(&blob[5..19], b"index-metadata");
    assert_eq!(&blob[19..23], &1u32.to_be_bytes());
    assert!(stock_verified_body(&blob, "index-metadata").is_ok());
    let n = blob.len();
    assert_eq!(&blob[n - 16..n - 8], &[0xC0, 0x28, 0x93, 0xE8, 0, 0, 0, 0]);
    let crc = crc32fast::hash(&blob[..n - 8]);
    assert_eq!(&blob[n - 8..], &u64::from(crc).to_be_bytes());
}

#[test]
fn another_codec_is_refused_on_read() {
    let blob = written("{}", "snapshot");
    assert!(read(&blob, "metadata").is_err());
}

#[test]
fn a_failed_write_leaves_the_buffer_as_it_was() {
    let mut out = b"keep".to_vec();
    let limits = StockEnvelopeLimits {
        decompressed_bytes: 64,
        smile: SmileLimits::default(),
    };
    assert!(stock_blob_from_json(b"{\"a\":", "snapshot", &limits, &mut out).is_err());
    assert_eq!(out, b"keep");
}

#[test]
fn a_codec_name_too_long_for_a_header_is_refused() {
    let mut out = Vec::new();
    let name = "c".repeat(200);
    assert!(stock_blob_from_json(b"{}", &name, &StockEnvelopeLimits::default(), &mut out).is_err());
    assert!(out.is_empty());
}

/// `OSS_WRITE_STOCK=dir` leaves the blobs for `CheckStockWrite.java`.
#[test]
#[allow(clippy::disallowed_methods)] // ARITH: a test writes a scratch directory.
fn the_written_blobs_are_left_for_the_java_check() {
    let Ok(dir) = std::env::var("OSS_WRITE_STOCK") else {
        return;
    };
    std::fs::create_dir_all(&dir).unwrap();
    let doc = r#"{"a":{"b":[1,2,3],"s":"x"},"t":true}"#;
    for codec in ["snapshot", "metadata", "index-metadata"] {
        std::fs::write(format!("{dir}/{codec}.dat"), written(doc, codec)).unwrap();
    }
    let long = format!("{{\"k\":\"{}\"}}", "y".repeat(1000));
    std::fs::write(format!("{dir}/snapshot-long.dat"), written(&long, "snapshot")).unwrap();
}
